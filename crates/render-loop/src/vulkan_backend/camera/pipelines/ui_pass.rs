//! Проход UI поверх компоновки видоискателя (сессия 11, решение 86).
//!
//! Что здесь живёт:
//! * [`UiPass`] — ленивый владелец `UiRenderer`: без установленного источника рендер НЕ создаётся
//!   (UI по умолчанию выключен, кадр видоискателя идёт как до сессии 11); создаётся на первом кадре с
//!   источником; пересоздаётся (лениво) при смене формата цели и цветового пространства дисплея (картинки UI
//!   переводятся в основные цвета экрана — они зашиты в конвейер); слоты отпускаются по fence слота кадра;
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
use gpu_vulkan::swapchain::{DisplayColorSpace, DisplayTargetInfo};
use ui_core::input::{PointScale, PointScaleRule};
use ui_draw::color::{DisplayColor, DisplayPrimaries};
use ui_draw::draw_list::LiveImage;
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_render_vulkan::{
    BackdropInput, BackdropPlacement, LiveImageInput, LiveImages, UiFrame, UiRenderError, UiRenderer, UiRendererConfig,
};

use super::super::ui_state::UiOverlayFrame;
use super::super::viewfinder_stats::FRAME_POINT_UI_DONE;
use super::{CameraPipelines, FrameTarget, LOG_TARGET, ScreenContent, logical_size};
use crate::config::UiOverlayConfig;
pub use tick::{ReadyUi, UiRecordOutcome, UiTick};

/// Ленивый владелец рендера UI под формат цели.
pub(super) struct UiPass {
    context: GpuContext,
    config: UiOverlayConfig,
    target_format: vk::Format,
    frame_slots: usize,
    /// Основные цвета дисплея, под которые создан (или будет создан) рендер.
    display_primaries: DisplayPrimaries,
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
            display_primaries: primaries_of(target.display_color_space),
            renderer: None,
        }
    }

    /// Новый формат цели: прежний рендер уничтожается (вызывающий уже дождался сабмитов цели), новый
    /// поднимется лениво на первом кадре с источником.
    pub(super) fn reset_for_format(&mut self, target: &DisplayTargetInfo) {
        self.renderer = None;
        self.target_format = target.format;
        self.frame_slots = target.frames_in_flight;
        self.display_primaries = primaries_of(target.display_color_space);
    }

    /// Совпадают ли основные цвета дисплея кадра с теми, под которые создан рендер.
    fn matches_display(&self, display_color_space: DisplayColorSpace) -> bool {
        self.display_primaries == primaries_of(display_color_space)
    }

    /// Fence слота пройден: буферы и наборы дескрипторов слота рендера UI снова можно переписывать.
    /// Без рендера нечего освобождать.
    pub(super) fn release_slot_after_fence(&mut self, slot: usize) -> Result<(), UiRenderError> {
        match self.renderer.as_mut() {
            Some(renderer) => renderer.release_slot_after_fence(slot),
            None => Ok(()),
        }
    }

    /// Записанный кадр слота не отправлен: рендер UI выбрасывает то, что этот кадр загрузил (без рендера — нечего).
    pub(super) fn frame_not_submitted(&mut self, slot: usize) -> Result<(), UiRenderError> {
        match self.renderer.as_mut() {
            Some(renderer) => renderer.frame_not_submitted(slot),
            None => Ok(()),
        }
    }

    /// Гарантирует, что рендер создан под текущий формат цели.
    fn ensure_renderer(&mut self) -> Result<&mut UiRenderer, UiRenderError> {
        if self.renderer.is_none() {
            let mut config = UiRendererConfig::new(self.target_format, self.frame_slots);
            config.limits = self.config.limits;
            config.pyramid = self.config.pyramid;
            config.display_primaries = self.display_primaries;
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

/// Основные цвета UI-картинок для цветового пространства дисплея (нейтральный тип gpu-vulkan → тип ui-draw:
/// сам `ui-draw` GPU не знает).
fn primaries_of(display_color_space: DisplayColorSpace) -> DisplayPrimaries {
    match display_color_space {
        DisplayColorSpace::Srgb => DisplayPrimaries::Srgb,
        DisplayColorSpace::DisplayP3 => DisplayPrimaries::DisplayP3,
    }
}

/// Пиксели на pt для логической ширины экрана по правилу масштаба решения 82 (ширина surface = эталонная
/// ширина в pt). Правило берётся из `ui-core` по умолчанию, не из константы здесь.
fn pixels_per_point(logical_width_px: u32) -> Result<f32, ui_core::input::PointScaleError> {
    let scale = PointScale::for_surface_width(PointScaleRule::default(), logical_width_px)?;
    Ok(scale.pixels_per_point() as f32)
}

/// Логический размер цели и масштаб pt этого кадра для запроса к источнику UI. Непригодный размер или масштаб — честный
/// пропуск кадра UI (причина в диагностике), `None`.
fn frame_geometry(frame: &FrameTarget, ui: &mut UiOverlayFrame<'_>) -> Option<(PxSize, f32)> {
    let logical = match logical_size(frame) {
        Ok(logical) => logical,
        Err(error) => {
            ui.state.note_frame_skipped(&format!("логический размер цели: {error}"));
            return None;
        }
    };
    let px_per_pt = match pixels_per_point(logical.width) {
        Ok(px_per_pt) => px_per_pt,
        Err(error) => {
            ui.state.note_frame_skipped(&format!("масштаб pt: {error}"));
            return None;
        }
    };
    Some((PxSize::new(logical.width as f32, logical.height as f32), px_per_pt))
}

/// Запрос к источнику UI на этот кадр: одинаков для записи и для подавленного кадра (источник не должен видеть
/// разницы, контракт «раз на показанный кадр»).
fn frame_request(
    ui: &UiOverlayFrame<'_>,
    logical_px: PxSize,
    px_per_pt: f32,
    continuity: BackdropContinuity,
) -> UiFrameRequest {
    UiFrameRequest {
        present_time: ui.present_time,
        logical_size: logical_px,
        px_per_pt,
        backdrop: continuity,
    }
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

    /// Кадр с подавленным рисованием UI (замер A/B, `SuppressibleGpuWork::UiOverlayDrawing`): источник получает свой
    /// `produce_frame` ровно как на обычном кадре (контракт «раз на показанный кадр», разбор касаний не ломается), но
    /// GPU-запись прохода UI не делается: ни создания рендера, ни пирамиды фона, ни загрузки экземпляров, ни меток
    /// времени. Картинка прибора UI не отдаётся (`note_picture_handed_to_ui` не зовётся): слот приборов
    /// ничем не занят и освобождается обычным путём. Без источника — ничего.
    #[cfg(test)]
    pub fn skip_ui_overlay_drawing(
        &self,
        frame: &FrameTarget,
        ui: &mut UiOverlayFrame<'_>,
        continuity: BackdropContinuity,
    ) {
        // Итог подготовки здесь не нужен: подавленный UI не записывается, а решение «рисовать ли тик» принимает
        // сессия показа (она зовёт `prepare_suppressed_ui_tick` сама).
        let _ = self.prepare_suppressed_ui_tick(frame, ui, continuity);
    }

    /// Подготовка UI тика при подавленном рисовании UI: то же, что [`Self::skip_ui_overlay_drawing`], но итог
    /// возвращается сессии показа (`Suppressed`, если источник дал список; список выбрасывается).
    pub fn prepare_suppressed_ui_tick(
        &self,
        frame: &FrameTarget,
        ui: &mut UiOverlayFrame<'_>,
        continuity: BackdropContinuity,
    ) -> UiTick {
        if !ui.state.is_active() {
            return UiTick::Inactive;
        }
        let Some((logical_px, px_per_pt)) = frame_geometry(frame, ui) else {
            return UiTick::Unavailable;
        };
        let request = frame_request(ui, logical_px, px_per_pt, continuity);
        // Список рисования выбрасывается: цена источника остаётся в замере, цена GPU — нет.
        if ui.state.produce_draw_list(&request).is_some() {
            ui.state.note_frame_suppressed();
            UiTick::Suppressed
        } else {
            UiTick::Unavailable
        }
    }

    /// Первый шаг UI тика: спрашивает у источника список рисования и проверяет, что рендер UI способен его
    /// нарисовать на этом кадре. Ничего не пишет в GPU (рендер создаётся лениво, если его ещё нет).
    ///
    /// Вызывается ровно один раз на тик, дошедший до кадра, ДО записи компоновки: по списку решается, нужен ли
    /// кадр вообще (решение 214), а запись ([`Self::record_prepared_ui`]) идёт только для нарисованного тика.
    /// `continuity` — что под UI на этом кадре (решает машина непрерывности).
    pub fn prepare_ui_tick(
        &mut self,
        frame: &FrameTarget,
        ui: &mut UiOverlayFrame<'_>,
        continuity: BackdropContinuity,
    ) -> UiTick {
        if !ui.state.is_active() {
            return UiTick::Inactive;
        }
        // Размеры считаем до всего остального: от них зависит запрос к источнику.
        let Some((logical_px, px_per_pt)) = frame_geometry(frame, ui) else {
            return UiTick::Unavailable;
        };
        if let Err(error) = self.ui.ensure_renderer() {
            tracing::error!(target: LOG_TARGET, %error, "рендер UI не создан: источник снят");
            ui.state.reject_source_after_setup_failure(&error);
            return UiTick::Unavailable;
        }
        // Рендер зашит под основные цвета дисплея. Пересоздать его посреди кадра нельзя (кадры в полёте ссылаются на
        // конвейеры), а рисовать картинки в чужом охвате — значит показывать неверные цвета: честный пропуск кадра UI.
        if !self.ui.matches_display(frame.display_color_space) {
            ui.state.note_frame_skipped(
                "цветовое пространство дисплея сменилось: рендер UI пересоздаётся при смене формата цели",
            );
            return UiTick::Unavailable;
        }
        let request = frame_request(ui, logical_px, px_per_pt, continuity);
        match ui.state.produce_draw_list(&request) {
            Some(draw_list) => UiTick::Ready(ReadyUi {
                draw_list,
                logical_px,
                px_per_pt,
                continuity,
            }),
            None => UiTick::Unavailable,
        }
    }

    /// Записывает UI этого кадра поверх уже записанной компоновки (или чёрного кадра): подготовка и запись за один
    /// вызов. Возвращает число отказов записи метки времени. Без установленного источника не делает ничего — ни
    /// создания рендера, ни команд, ни меток: кадр остаётся таким, каким был до сессии 11.
    ///
    /// Боевой путь (сессия показа) вызывает шаги по отдельности — [`Self::prepare_ui_tick`] и
    /// [`Self::record_prepared_ui`]; этот вход нужен стендам, которым решение «рисовать ли тик» не нужно.
    #[cfg(test)]
    pub fn record_ui_overlay(
        &mut self,
        frame: &FrameTarget,
        ui: &mut UiOverlayFrame<'_>,
        screen: &ScreenContent,
        serial: super::super::SubmissionSerial,
        continuity: BackdropContinuity,
    ) -> u64 {
        let tick = self.prepare_ui_tick(frame, ui, continuity);
        self.record_prepared_ui(frame, ui, &tick, screen, serial)
            .timing_failures
    }

    /// Второй шаг UI тика: записывает подготовленный список в команды кадра.
    ///
    /// `screen` — что компоновка положила на экран: изображение видоискателя даёт живой фон стеклу,
    /// чёрный кадр (скрытый кадр AWB, отказ ISP или компоновщика) — фон `Absent` чёрного цвета.
    /// `serial` — номер сабмита этого кадра (для подложки стекла). `tick` — итог [`Self::prepare_ui_tick`]:
    /// `Inactive` и `Unavailable` ничего не записывают.
    pub fn record_prepared_ui(
        &mut self,
        frame: &FrameTarget,
        ui: &mut UiOverlayFrame<'_>,
        tick: &UiTick,
        screen: &ScreenContent,
        serial: super::super::SubmissionSerial,
    ) -> UiRecordOutcome {
        let UiTick::Ready(ready) = tick else {
            return UiRecordOutcome::NOT_RECORDED;
        };
        let draw_list = &ready.draw_list;
        let logical_px = ready.logical_px;

        // Фон под стеклом. Подложка берётся заново на каждый кадр (контракт `gpu_vulkan::backdrop`).
        // Прямоугольник фона — тот же, что скомпоновал видоискатель (посчитан один раз на кадр).
        // Без изображения камеры (чёрный кадр) стеклу фоном служит чёрный.
        let backdrop_source = match screen {
            // Живое изображение: подложка читает содержимое кадра (не всю картинку ёмкости) и зеркалится так же,
            // как скомпонованное на экран, — стекло преломляет то, что видит пользователь.
            ScreenContent::Viewfinder { .. } => self
                .isp
                .backdrop_source(gpu_vulkan::backdrop::SubmissionSerial(serial.0))
                .and_then(|source| {
                    let window = source.window().with_mirror(self.view_live_mirror());
                    source
                        .with_window(window)
                        .inspect_err(|error| {
                            tracing::error!(target: LOG_TARGET, %error, "окно подложки стекла отвергнуто");
                        })
                        .ok()
                }),
            // Удержанный кадр: подложка из него, с тем же окном (зеркало и увеличение), что и на экране.
            ScreenContent::Held { .. } => self.held_backdrop_source(),
            ScreenContent::Black { .. } => None,
        };
        let backdrop = match (&backdrop_source, screen) {
            (Some(source), ScreenContent::Viewfinder { destination } | ScreenContent::Held { destination }) => {
                BackdropInput::Live {
                    source,
                    placement: BackdropPlacement {
                        destination: px_rect(*destination),
                    },
                }
            }
            _ => BackdropInput::Absent {
                fill: DisplayColor::BLACK,
            },
        };

        // Живые картинки кадра: картинка цветового прибора (14h), если она записана хотя бы раз. Это верно и для кадра
        // перекомпоновки: картинка последнего нового кадра остаётся валидной и в раскладке «чтение фрагментом».
        let scope_entries;
        let live_images = match self.scopes.picture_for_ui() {
            Some(view) => {
                scope_entries = [(LiveImage::ColorScope, LiveImageInput { view })];
                // Слот держит набор приборов, пока GPU не закончит этот кадр.
                self.scopes.note_picture_handed_to_ui(frame.ring_slot);
                LiveImages::new(&scope_entries)
            }
            None => LiveImages::none(),
        };
        let ui_frame = UiFrame {
            draw_list,
            logical_size: logical_px,
            surface_transform: frame.transform,
            backdrop,
            live_images,
        };
        let target = ColorAttachment::from_display_target(frame.pattern_target, frame.format);
        let recorded = match self.ui.renderer.as_mut() {
            Some(renderer) => renderer.record_frame(frame.command_buffer, frame.ring_slot, target, &ui_frame),
            None => return UiRecordOutcome::NOT_RECORDED,
        };
        match recorded {
            Ok(report) => {
                ui.state.note_drawn(&report);
                UiRecordOutcome {
                    timing_failures: self.point(frame, FRAME_POINT_UI_DONE, TimestampStage::AllCommandsDone),
                    drawn: true,
                }
            }
            Err(error) => {
                if matches!(error, UiRenderError::Gpu(_)) {
                    tracing::error!(target: LOG_TARGET, %error, "сбой GPU при записи UI: команды кадра могли записаться частично");
                }
                ui.state.note_render_failure(&error);
                UiRecordOutcome::NOT_RECORDED
            }
        }
    }
}

mod tick;

#[cfg(test)]
mod suppression_tests;
#[cfg(test)]
mod unchanged_pixels_tests;
