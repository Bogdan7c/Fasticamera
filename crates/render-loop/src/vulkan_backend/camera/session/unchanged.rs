//! Решение «рисовать ли этот тик» (решение 214, сессия 16a): тик, на котором ничего не изменилось с последнего
//! показанного кадра, не записывается и не показывается — экран продолжает держать прежний кадр.
//!
//! # Почему экран остаётся прежним
//! Цепочка показа — FIFO (`VK_PRESENT_MODE_FIFO_KHR`, единственный режим, который есть у любой реализации).
//! Показ кадра ставит картинку в очередь; когда очередь опустела, дисплей продолжает показывать последнюю
//! поставленную картинку, а не предпоследнюю: глубина очереди значения не имеет, все поставленные картинки
//! показываются по порядку, и на экране остаётся самая новая. Поэтому «прежний кадр» — это всегда ПОСЛЕДНИЙ кадр,
//! принятый очередью показа. Именно его содержимое запоминает [`UnchangedFrameGate`] (`Baseline`): запоминается
//! только кадр, чей сабмит принят и чей показ вернул успех.
//!
//! # Что значит «не изменилось»
//! Пиксели тика без нового кадра камеры определяются так: изображение ISP (меняется только новым кадром камеры) +
//! настройки видоискателя (команды; любая принятая команда меняет `scene_revision`) + UI тика (список рисования,
//! размер, масштаб, фон под стеклом). Список сравнивается целиком (`DrawList: PartialEq` по полям; картинки в
//! памяти — по номеру версии), без хэшей: равенство точное, ложных «совпало» не бывает.
//!
//! # Чего здесь НЕТ
//! Решение берёт только чистые входы и ничего не пишет в GPU: тестируется без Vulkan. Источник UI спрашивает
//! сессия показа ДО решения (на каждом тике), потому что его контракт — раз на тик (касания, анимации).

use gpu_vulkan::swapchain::{DisplayFrame, PresentOutcome as GpuPresentOutcome, SwapchainError};
use ui_draw::frame_source::BackdropContinuity;

use super::transition::backdrop_of;
use super::{CameraSession, Content, Prepared};
use crate::config::{UnchangedFrameConfig, UnchangedFramePolicy};
use crate::viewfinder::{DrawReason, GpuWorkSuppression, SuppressibleGpuWork, ViewfinderPainter};
use crate::vulkan_backend::camera::continuity::{Composed, Screen, StreamEvent};
use crate::vulkan_backend::camera::lease::FrameChange;
use crate::vulkan_backend::camera::pipelines::{CameraPipelines, FrameTarget, UiTick};
use crate::vulkan_backend::camera::ui_state::UiOverlayFrame;

/// Что за тик предстоит: от этого зависит, можно ли его вообще рассматривать как пропускаемый.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TickKind {
    /// Новый кадр камеры (ISP) или кадр отладочной мозаики: картинка изменилась.
    NewCameraFrame,
    /// Перекомпоновка живого изображения без нового кадра: поток тихий, удержания нет, возвращать камере нечего.
    QuietLiveRecompose,
    /// Замер A/B подавляет работу GPU.
    GpuWorkSuppressed,
    /// Всё остальное: удержанный кадр, чёрный, переход, возврат кадра брекетинга, чистка мозаики.
    Other,
}

/// Решение по тику.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameVerdict {
    /// Тик не записывается и не показывается.
    Skip,
    /// Тик рисуется по этой причине.
    Draw(DrawReason),
}

/// Что известно о качестве только что отправленного кадра: по нему решается, годится ли он как эталон сравнения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrawnFrame {
    /// Сабмит принят, показ принят, на экран легло живое изображение видоискателя, а UI (если был) записан целиком.
    CleanLive,
    /// Любой другой исход (ошибка записи или показа, чёрный, удержанный, оборванный UI): эталона нет.
    Unreliable,
}

/// Кадр, чьё содержимое сейчас на экране (последний принятый очередью показа).
#[derive(Debug)]
struct Baseline {
    /// Версия настроек видоискателя, при которой кадр нарисован.
    scene_revision: u64,
    /// UI кадра: список рисования и всё, от чего он зависит (или «UI не было»).
    ui: UiTick,
}

/// Владелец решения «рисовать ли тик» и эталона сравнения. Живёт в сессии показа одной цепочки.
pub(super) struct UnchangedFrameGate {
    config: UnchangedFrameConfig,
    baseline: Option<Baseline>,
    /// Версия настроек видоискателя: растёт на каждую принятую команду. Сравнивается с версией эталона.
    scene_revision: u64,
    /// Сколько тиков подряд уже пропущено.
    skip_run: u32,
}

impl UnchangedFrameGate {
    pub(super) fn new(config: UnchangedFrameConfig) -> Self {
        Self {
            config,
            baseline: None,
            scene_revision: 0,
            skip_run: 0,
        }
    }

    /// Решает, рисовать ли тик. `ui` — UI этого тика (источник уже спрошен). Порядок проверок — порядок
    /// [`DrawReason`]: первая сработавшая причина и есть причина отрисовки.
    pub(super) fn judge(&mut self, tick: TickKind, ui: &UiTick) -> FrameVerdict {
        let verdict = self.decide(tick, ui);
        if verdict == FrameVerdict::Skip {
            self.skip_run += 1;
        }
        verdict
    }

    fn decide(&self, tick: TickKind, ui: &UiTick) -> FrameVerdict {
        if self.config.policy == UnchangedFramePolicy::AlwaysRedraw {
            return FrameVerdict::Draw(DrawReason::PolicyAlwaysRedraw);
        }
        match tick {
            TickKind::GpuWorkSuppressed => return FrameVerdict::Draw(DrawReason::GpuWorkSuppressed),
            TickKind::NewCameraFrame => return FrameVerdict::Draw(DrawReason::NewCameraFrame),
            TickKind::Other => return FrameVerdict::Draw(DrawReason::NotQuietLiveRecompose),
            TickKind::QuietLiveRecompose => {}
        }
        if matches!(ui, UiTick::Unavailable | UiTick::Suppressed) {
            return FrameVerdict::Draw(DrawReason::UiUnavailable);
        }
        let Some(baseline) = &self.baseline else {
            return FrameVerdict::Draw(DrawReason::NoBaseline);
        };
        if baseline.scene_revision != self.scene_revision {
            return FrameVerdict::Draw(DrawReason::SceneChanged);
        }
        if baseline.ui != *ui {
            return FrameVerdict::Draw(DrawReason::UiChanged);
        }
        if self.skip_run >= self.config.max_consecutive_skips {
            return FrameVerdict::Draw(DrawReason::SkipRunLimit);
        }
        FrameVerdict::Skip
    }

    /// Нарисованный тик отправлен: если кадр надёжный, его UI становится эталоном; иначе эталона нет. В любом
    /// случае серия пропусков кончилась.
    pub(super) fn note_drawn(&mut self, ui: UiTick, frame: DrawnFrame) {
        self.skip_run = 0;
        self.baseline = match (frame, &ui) {
            (DrawnFrame::CleanLive, UiTick::Inactive | UiTick::Ready(_)) => Some(Baseline {
                scene_revision: self.scene_revision,
                ui,
            }),
            // «Нет UI из-за отказа» на экране выглядит как неизвестно что: эталоном быть не может.
            (DrawnFrame::CleanLive, UiTick::Unavailable | UiTick::Suppressed) | (DrawnFrame::Unreliable, _) => None,
        };
    }

    /// Содержимое экрана неизвестно (ошибка записи или показа, пересоздание цепочки, потеря кадра): сравнивать
    /// не с чем, следующий тик нарисуется.
    pub(super) fn invalidate(&mut self) {
        self.baseline = None;
        self.skip_run = 0;
    }

    /// Принята команда видоискателя: всё, что нарисовано до неё, могло устареть.
    pub(super) fn note_scene_changed(&mut self) {
        self.scene_revision += 1;
    }
}

impl CameraSession {
    /// UI этого тика: источник спрашивается ровно раз, ДО записи кадра (см. `UiTick`). Какой фон под UI — решает
    /// машина непрерывности; чёрный кадр и отладочная мозаика получают свои правила, те же, что у записи кадра.
    pub(super) fn prepare_ui_tick(
        &mut self,
        pipelines: &mut CameraPipelines,
        frame: &DisplayFrame,
        ui: &mut UiOverlayFrame<'_>,
        prepared: &Prepared,
    ) -> UiTick {
        let transition = prepared.transition.as_ref();
        let continuity = match &prepared.content {
            // Отладочная мозаика UI не знает: чистый кадр нужен для A/B-замера.
            Content::Mosaic { .. } => return UiTick::Inactive,
            // Чёрный кадр без источника UI или под мозаикой остаётся чёрным; с UI под ним фон `NoCameraFrame`.
            Content::Black if self.settings.painter != ViewfinderPainter::Isp => return UiTick::Inactive,
            Content::Black => backdrop_of(transition, BackdropContinuity::NoCameraFrame),
            Content::IspNew { .. } | Content::Recompose => backdrop_of(transition, BackdropContinuity::Live),
        };
        let target = FrameTarget::of(frame);
        // Замер A/B (16a): подавленный UI не записывается в GPU, но источник по-прежнему получает свой кадр.
        if self
            .settings
            .gpu_work_suppression
            .suppresses(SuppressibleGpuWork::UiOverlayDrawing)
        {
            pipelines.prepare_suppressed_ui_tick(&target, ui, continuity)
        } else {
            pipelines.prepare_ui_tick(&target, ui, continuity)
        }
    }

    /// Решение по тику (см. модуль) и его учёт в счётчиках видоискателя. Серия пропусков и эталон ведутся внутри
    /// [`UnchangedFrameGate`].
    pub(super) fn judge_tick(&mut self, prepared: &Prepared, ui_tick: &UiTick) -> FrameVerdict {
        let kind = classify_tick(prepared, self.settings.painter, self.settings.gpu_work_suppression);
        let verdict = self.unchanged.judge(kind, ui_tick);
        match verdict {
            FrameVerdict::Draw(reason) => self.viewfinder.counters_mut().unchanged_frames.note_drawn(reason),
            FrameVerdict::Skip => {
                self.viewfinder.counters_mut().unchanged_frames.note_skipped();
                // Зритель снова видит тот же кадр камеры: повтор не нарисован, но в метрику «видимые повторы» входит.
                self.stats.camera_frame_repeats_held += 1;
            }
        }
        verdict
    }

    /// Показ возобновлён после паузы: эталон «что на экране» сброшен, следующий тик рисуется.
    pub fn forget_shown_frame(&mut self) {
        self.unchanged.invalidate();
    }

    /// Нарисованный тик отправлен очереди: решает, годится ли он эталоном «что сейчас на экране». Годится только кадр,
    /// чей показ очередь приняла, с живым изображением видоискателя, с полностью записанным UI и без замера A/B.
    pub(super) fn note_frame_sent(
        &mut self,
        ui_tick: UiTick,
        composed: Composed,
        ui_complete: bool,
        outcome: &Result<GpuPresentOutcome, SwapchainError>,
    ) {
        let queued_for_display = matches!(
            outcome,
            Ok(GpuPresentOutcome::Presented { .. } | GpuPresentOutcome::Suboptimal { .. })
        );
        let clean = composed == Composed::Live
            && ui_complete
            && queued_for_display
            && self.settings.gpu_work_suppression.is_none();
        self.unchanged.note_drawn(
            ui_tick,
            if clean {
                DrawnFrame::CleanLive
            } else {
                DrawnFrame::Unreliable
            },
        );
    }
}

/// Классифицирует тик по решению `prepare`: можно ли его рассматривать как пропускаемую перекомпоновку.
///
/// Пропускаемой считается только перекомпоновка ЖИВОГО изображения ISP в тихом потоке: без нового кадра, без
/// возврата кадра камере (`Keep`), без удержания и конца удержания, с событием потока `Quiet`. Событие
/// `Unpresentable` (кадр пришёл, но показать его нельзя) меняет машину непрерывности при фиксации кадра, поэтому
/// такой тик всегда рисуется.
pub(super) fn classify_tick(
    prepared: &Prepared,
    painter: ViewfinderPainter,
    suppression: GpuWorkSuppression,
) -> TickKind {
    if !suppression.is_none() {
        return TickKind::GpuWorkSuppressed;
    }
    match &prepared.content {
        Content::IspNew { .. } | Content::Mosaic { .. } => TickKind::NewCameraFrame,
        Content::Black => TickKind::Other,
        Content::Recompose => {
            let quiet_live = painter == ViewfinderPainter::Isp
                && prepared.change == FrameChange::Keep
                && prepared.new_lease.is_none()
                && !prepared.stream_inactive
                && prepared.transition.as_ref().is_some_and(|transition| {
                    transition.plan.screen == Screen::Live
                        && transition.plan.begins_hold.is_none()
                        && transition.plan.ends_hold.is_none()
                        && matches!(transition.event, StreamEvent::Quiet)
                });
            if quiet_live {
                TickKind::QuietLiveRecompose
            } else {
                TickKind::Other
            }
        }
    }
}

#[cfg(test)]
#[path = "unchanged_tests.rs"]
mod tests;
