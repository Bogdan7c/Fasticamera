//! Проход UI поверх компоновки видоискателя (сессия 11, решение 86).
//!
//! Что здесь живёт:
//! * [`UiPass`] — ленивый владелец `UiRenderer`: без установленного источника рендер НЕ создаётся
//!   (UI по умолчанию выключен, кадр видоискателя идёт как до сессии 11); создаётся на первом кадре с
//!   источником; пересоздаётся (лениво) при смене формата цели; слоты отпускаются по fence слота кадра;
//! * [`CameraPipelines::record_ui_overlay`] — запись UI одного кадра: источник → список рисования → фон
//!   (живой видоискатель или чёрный) → `record_frame_marked` в цель кадра → метка «UI готов».
//!
//! Инвариант: UI не может уронить показ камеры. Отказ создания рендера, источника или записи кадра
//! считается в [`UiOverlayState`](super::super::ui_state::UiOverlayState) и оставляет кадр видоискателя как
//! есть. Исключение, о котором надо знать: ошибка `UiRenderError::Gpu` происходит уже ПОСЛЕ начала записи
//! команд (контракт `UiRenderer`), такой кадр логируется как ошибка; остальные ошибки безопасны.
//!
//! Рендер живёт в `CameraPipelines`, а не в состоянии бэкенда: кадры в полёте ссылаются на его конвейеры,
//! и уничтожать их можно только после того, как цель освободила GPU (то же правило, что у компоновщика и ISP).

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::TimestampStage;
use gpu_vulkan::raster::ColorAttachment;
use gpu_vulkan::swapchain::{DisplayFrame, DisplayTargetInfo};
use ui_core::input::{PointScale, PointScaleRule};
use ui_draw::color::DisplayColor;
use ui_draw::frame_source::UiFrameRequest;
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_render_vulkan::{BackdropInput, BackdropPlacement, UiFrame, UiRenderError, UiRenderer, UiRendererConfig};

use super::super::ui_state::UiOverlayFrame;
use super::super::viewfinder_stats::FRAME_POINT_UI_DONE;
use super::{CameraPipelines, LOG_TARGET, PlaneSize, ScreenContent, logical_size, viewfinder_layout};
use crate::config::{UiOverlayConfig, ViewfinderPlacement};

/// Ленивый владелец рендера UI под формат цели.
pub(super) struct UiPass {
    context: GpuContext,
    config: UiOverlayConfig,
    target_format: vk::Format,
    frame_slots: usize,
    /// `None` — рендер ещё не нужен (источника не было) или сброшен сменой формата цели.
    renderer: Option<UiRenderer>,
}

impl UiPass {
    /// Пустой проход под формат и число слотов цели. Объекты GPU не создаются.
    pub(super) fn new(context: &GpuContext, target: &DisplayTargetInfo, config: &UiOverlayConfig) -> Self {
        Self {
            context: context.clone(),
            config: *config,
            target_format: target.format,
            frame_slots: target.frames_in_flight,
            renderer: None,
        }
    }

    /// Новый формат цели: прежний рендер уничтожается (вызывающий уже дождался сабмитов цели), новый
    /// поднимется лениво на первом кадре с источником.
    pub(super) fn reset_for_format(&mut self, target: &DisplayTargetInfo) {
        self.renderer = None;
        self.target_format = target.format;
        self.frame_slots = target.frames_in_flight;
    }

    /// Fence слота пройден: буферы и наборы дескрипторов слота рендера UI снова можно переписывать.
    /// Без рендера нечего освобождать.
    pub(super) fn release_slot_after_fence(&mut self, slot: usize) -> Result<(), UiRenderError> {
        match self.renderer.as_mut() {
            Some(renderer) => renderer.release_slot_after_fence(slot),
            None => Ok(()),
        }
    }

    /// Гарантирует, что рендер создан под текущий формат цели.
    fn ensure_renderer(&mut self) -> Result<&mut UiRenderer, UiRenderError> {
        if self.renderer.is_none() {
            let mut config = UiRendererConfig::new(self.target_format, self.frame_slots);
            config.limits = self.config.limits;
            config.pyramid = self.config.pyramid;
            self.renderer = Some(UiRenderer::new(&self.context, &config)?);
        }
        self.renderer.as_mut().ok_or_else(|| {
            // Недостижимо: рендер только что создан выше. Не паникуем — честная ошибка кадра.
            UiRenderError::Gpu(GpuError::InvalidState {
                what: "рендер UI не создан".to_owned(),
            })
        })
    }
}

/// Пиксели на pt для логической ширины экрана по правилу масштаба решения 82 (ширина surface = эталонная
/// ширина в pt). Правило берётся из `ui-core` по умолчанию, не из константы здесь.
fn pixels_per_point(logical_width_px: u32) -> Result<f32, ui_core::input::PointScaleError> {
    let scale = PointScale::for_surface_width(PointScaleRule::default(), logical_width_px)?;
    Ok(scale.pixels_per_point() as f32)
}

/// Прямоугольник в логических пикселях -> прямоугольник UI.
fn px_rect(rect: gpu_vulkan::compositor::LogicalRect) -> PxRect {
    PxRect {
        origin: PxPoint::new(rect.x as f32, rect.y as f32),
        size: PxSize::new(rect.width as f32, rect.height as f32),
    }
}

impl CameraPipelines {
    /// Создать рендер UI заранее (между кадрами), если его ещё нет. Без установленного источника
    /// вызывающий этого не делает: продукт без UI объектов рендера UI не держит.
    pub fn prepare_ui_overlay(&mut self) -> Result<(), UiRenderError> {
        self.ui.ensure_renderer().map(|_renderer| ())
    }

    /// Записывает UI этого кадра поверх уже записанной компоновки (или чёрного кадра). Возвращает число
    /// отказов записи метки времени. Без установленного источника не делает ничего — ни создания рендера,
    /// ни команд, ни меток: кадр остаётся таким, каким был до сессии 11.
    ///
    /// `screen` — что компоновка положила на экран: изображение видоискателя даёт живой фон стеклу,
    /// чёрный кадр (скрытый кадр AWB, отказ ISP или компоновщика) — фон `Absent` чёрного цвета.
    /// `serial` — номер сабмита этого кадра (для подложки стекла).
    pub fn record_ui_overlay(
        &mut self,
        frame: &DisplayFrame,
        ui: &mut UiOverlayFrame<'_>,
        screen: &ScreenContent,
        serial: super::super::SubmissionSerial,
        placement: ViewfinderPlacement,
    ) -> u64 {
        if !ui.state.is_active() {
            return 0;
        }
        // Размеры считаем до всего остального: от них зависит запрос к источнику.
        let logical = match logical_size(frame) {
            Ok(logical) => logical,
            Err(error) => {
                ui.state.note_frame_skipped(&format!("логический размер цели: {error}"));
                return 0;
            }
        };
        let px_per_pt = match pixels_per_point(logical.width) {
            Ok(px_per_pt) => px_per_pt,
            Err(error) => {
                ui.state.note_frame_skipped(&format!("масштаб pt: {error}"));
                return 0;
            }
        };
        if let Err(error) = self.ui.ensure_renderer() {
            tracing::error!(target: LOG_TARGET, %error, "рендер UI не создан: источник снят");
            ui.state.reject_source_after_setup_failure(&error);
            return 0;
        }
        let logical_px = PxSize::new(logical.width as f32, logical.height as f32);
        let request = UiFrameRequest {
            present_time: ui.present_time,
            logical_size: logical_px,
            px_per_pt,
        };
        let Some(draw_list) = ui.state.produce_draw_list(&request) else {
            return 0;
        };

        // Фон под стеклом. Подложка берётся заново на каждый кадр (контракт `gpu_vulkan::backdrop`).
        let backdrop_source = match screen {
            ScreenContent::Viewfinder => self
                .isp
                .backdrop_source(gpu_vulkan::backdrop::SubmissionSerial(serial.0)),
            ScreenContent::Black { .. } => None,
        };
        let destination = backdrop_source.as_ref().and_then(|source| {
            let size = source.extent();
            viewfinder_layout(
                frame,
                PlaneSize {
                    width: size.width,
                    height: size.height,
                },
                placement,
            )
            .inspect_err(|error| {
                tracing::warn!(target: LOG_TARGET, %error, "прямоугольник фона для UI не посчитан: стекло над чёрным");
            })
            .ok()
            .map(|layout| px_rect(layout.destination))
        });
        let backdrop = match (&backdrop_source, destination) {
            (Some(source), Some(destination)) => BackdropInput::Live {
                source,
                placement: BackdropPlacement { destination },
            },
            _ => BackdropInput::Absent {
                fill: DisplayColor::BLACK,
            },
        };

        let ui_frame = UiFrame {
            draw_list: &draw_list,
            logical_size: logical_px,
            surface_transform: frame.transform(),
            backdrop,
        };
        let target = ColorAttachment::from_display_target(frame.pattern_target(), frame.format());
        let recorded = match self.ui.renderer.as_mut() {
            Some(renderer) => renderer.record_frame(frame.command_buffer(), frame.ring_slot(), target, &ui_frame),
            None => return 0,
        };
        match recorded {
            Ok(report) => {
                ui.state.note_drawn(&report);
                self.point(frame, FRAME_POINT_UI_DONE, TimestampStage::AllCommandsDone)
            }
            Err(error) => {
                if matches!(error, UiRenderError::Gpu(_)) {
                    tracing::error!(target: LOG_TARGET, %error, "сбой GPU при записи UI: команды кадра могли записаться частично");
                }
                ui.state.note_render_failure(&error);
                0
            }
        }
    }
}
