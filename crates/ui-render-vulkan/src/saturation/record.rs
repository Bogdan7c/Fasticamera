//! Запись одного кадра работы: цель → компоновщик (или очистка) → барьер → проход UI, с метками времени.

use gpu_vulkan::backdrop::BackdropSource;
use gpu_vulkan::color_target::{ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, LogicalRect};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raster::{AttachmentLoad, RasterPass, record_between_color_passes};
use gpu_vulkan::saturation::FrameRecording;
use gpu_vulkan::test_pattern::{PatternTarget, SurfaceTransform};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};

use super::UiFrameSaturation;
use super::pass_timer::PassPoint;
use crate::error::UiRenderError;
use crate::frame::{BackdropInput, BackdropPlacement, FrameMarker, UiFrame};
use crate::live_images::LiveImages;

/// Цвет очистки цели в режиме `UiOnly` (как у `tests/timing.rs`: непрозрачный чёрный).
const UI_ONLY_CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Ошибка рендера UI → `GpuError` (трейт стенда возвращает только `GpuError`). Ошибка GPU проходит как есть,
/// остальные превращаются в `InvalidState` с полным текстом причины.
pub(super) fn renderer_error_as_gpu_error(error: UiRenderError) -> GpuError {
    match error {
        UiRenderError::Gpu(gpu_error) => gpu_error,
        other => GpuError::InvalidState {
            what: format!("рендер UI в стенде насыщения: {other}"),
        },
    }
}

impl UiFrameSaturation {
    /// Записывает кадр слота из `recording` (End и отправку делает стенд).
    pub(super) fn record_one_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        let slot = recording.slot().index();
        if slot >= self.targets.len() {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "слот стенда {slot} вне 0..{}: число слотов адаптера должно совпадать с настройкой стенда",
                    self.targets.len()
                ),
            });
        }
        // Ошибка прошлого освобождения слота важнее нового кадра: результаты прогона уже под сомнением.
        if let Some(error) = self.pending_release_failure.take() {
            return Err(error);
        }
        let command_buffer = recording.command_buffer();
        self.timer.record_reset(command_buffer, slot)?;
        self.timer.record_point(command_buffer, slot, PassPoint::Start)?;

        let previous_use = if self.target_used[slot] {
            ColorTargetPreviousUse::Rendered
        } else {
            ColorTargetPreviousUse::NotUsedYet
        };
        // Старое содержимое цели не нужно: каждый кадр рисуется заново с нуля.
        self.targets[slot].record_prepare_for_render(command_buffer, previous_use, ContentOnPrepare::Discard)?;

        // Номер кадра фона решает, перестроится ли пирамида (тот же механизм, что у продукта).
        let backdrop = self.scene.backdrop.backdrop_for_frame(
            self.config
                .backdrop_cadence
                .backdrop_frame_number(recording.frame_index()),
        );
        if self.config.composition.includes_compositor() {
            self.record_compositor_pass(command_buffer, slot, &backdrop)?;
        } else {
            self.record_clear_pass(command_buffer, slot)?;
        }
        self.timer
            .record_point(command_buffer, slot, PassPoint::FirstPassDone)?;

        if self.config.composition.includes_ui() {
            // Второй цветовой проход пишет в ту же цель поверх первого: нужен барьер (иначе hazard «чтение после записи»).
            record_between_color_passes(&self.context, command_buffer);
            self.record_ui_pass(command_buffer, slot, &backdrop)?;
        }

        self.target_used[slot] = true;
        self.frames_recorded += 1;
        self.last_recorded_slot = Some(slot);
        Ok(())
    }

    /// Компоновщик: картинка «видоискателя» в прямоугольник сцены, остальное чёрное (как у продукта).
    fn record_compositor_pass(
        &mut self,
        command_buffer: ash::vk::CommandBuffer,
        slot: usize,
        backdrop: &BackdropSource,
    ) -> Result<(), GpuError> {
        let target = &self.targets[slot];
        let request = CompositeRequest {
            source: backdrop.view(),
            transform: SurfaceTransform::Identity,
            destination: self.scene.viewfinder,
            background: BackgroundColor::BLACK,
            frame_slot: slot,
        };
        self.compositor.record_windowed(
            command_buffer,
            PatternTarget {
                view: target.sampled_view().view,
                extent: target.extent(),
            },
            &request,
            &backdrop.window(),
        )
    }

    /// Очистка цели (режим `UiOnly`: компоновщика нет, UI нужна определённая подложка под `Preserve`).
    fn record_clear_pass(&self, command_buffer: ash::vk::CommandBuffer, slot: usize) -> Result<(), GpuError> {
        let target = &self.targets[slot];
        RasterPass::begin(
            &self.context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(UI_ONLY_CLEAR_COLOR),
        )?
        .end();
        Ok(())
    }

    /// Проход UI поверх цели: пирамида фона и список рисования сцены. Метки — через `FrameMarker`.
    fn record_ui_pass(
        &mut self,
        command_buffer: ash::vk::CommandBuffer,
        slot: usize,
        backdrop: &BackdropSource,
    ) -> Result<(), GpuError> {
        let target = &self.targets[slot];
        let extent = target.extent();
        let frame = UiFrame {
            draw_list: &self.scene.draw_list,
            logical_size: PxSize::new(extent.width as f32, extent.height as f32),
            surface_transform: SurfaceTransform::Identity,
            backdrop: BackdropInput::Live {
                source: backdrop,
                placement: BackdropPlacement {
                    destination: pixel_rect(self.scene.viewfinder),
                },
            },
            live_images: LiveImages::new(&self.scene.live_images),
        };
        let timer = &mut self.timer;
        // Ошибку метки нельзя вернуть из замыкания: запоминаем первую и отдаём после записи кадра.
        let mut marker_error: Option<GpuError> = None;
        let result = self.renderer.record_frame_marked(
            command_buffer,
            slot,
            target.attachment(),
            &frame,
            &mut |marker, marker_command_buffer| {
                let point = match marker {
                    // «Начало» у нас своё (до подготовки цели); проход UI открывается позже.
                    FrameMarker::Start => return,
                    FrameMarker::PyramidDone => PassPoint::PyramidDone,
                    FrameMarker::UiPassDone => PassPoint::UiDone,
                };
                if let Err(error) = timer.record_point(marker_command_buffer, slot, point) {
                    marker_error.get_or_insert(error);
                }
            },
        );
        result.map_err(renderer_error_as_gpu_error)?;
        marker_error.map_or(Ok(()), Err)
    }
}

/// Прямоугольник сцены в px цели → прямоугольник размещения фона для UI.
fn pixel_rect(rect: LogicalRect) -> PxRect {
    PxRect::new(
        PxPoint::new(rect.x as f32, rect.y as f32),
        PxSize::new(rect.width as f32, rect.height as f32),
    )
}
