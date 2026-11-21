//! Склейка сессии показа с машиной непрерывности (сессия 13): решение до записи команд, фиксация по факту
//! записи и учёт в диагностике. Сама машина — `camera::continuity` (чистая, без GPU); объекты GPU удержанного
//! кадра — `pipelines::held`.
//!
//! Порядок кадра: `plan_transition` (в `prepare`) → запись → `commit_transition` (состояние машины и
//! владение удержанной картинкой) → после принятого сабмита `note_transition_presented` (счётчики и метка
//! кадра). Метки и счётчики пишутся ТОЛЬКО для кадров, принятых очередью: «показан» значит «сабмит принят».

use shell_contract::clock::MonotonicNanos;
use ui_draw::frame_source::BackdropContinuity;

use super::CameraSession;
use crate::viewfinder::{FrameCropReport, FrameRecord};
use crate::vulkan_backend::camera::continuity::{
    Composed, FrameOutcome, FramePlan, HoldEvent, PictureMeta, Screen, StreamEvent,
};
use crate::vulkan_backend::camera::pipelines::{CameraPipelines, ShowFrameAs};

/// Решение непрерывности для одного кадра показа и то, что нужно для его фиксации.
#[derive(Debug, Clone)]
pub(super) struct TransitionFrame {
    /// Что произошло с потоком.
    pub event: StreamEvent,
    /// Решение: что на экран, начать ли и закончить ли удержание.
    pub plan: FramePlan,
    /// Метаданные нового кадра, записываемого в ISP (`None` — нового кадра нет).
    pub picture: Option<PictureMeta>,
    /// Кадрирование этого кадра (для диагностики).
    pub crop: Option<FrameCropReport>,
}

impl TransitionFrame {
    /// Что записать на экран.
    pub(super) fn show(&self) -> ShowFrameAs {
        match self.plan.screen {
            Screen::Live => ShowFrameAs::Live,
            Screen::Held => ShowFrameAs::Held {
                magnification: self.plan.magnification,
            },
            Screen::Black => ShowFrameAs::Black,
        }
    }
}

/// Фон под UI для кадра: решение непрерывности или, если кадр им не ведётся (отладочная мозаика), `default`.
pub(super) fn backdrop_of(transition: Option<&TransitionFrame>, default: BackdropContinuity) -> BackdropContinuity {
    transition.map_or(default, |transition| transition.plan.backdrop())
}

impl CameraSession {
    /// Решение непрерывности для кадра, который будет показан в `now`.
    pub(super) fn plan_transition(
        &self,
        now: MonotonicNanos,
        event: StreamEvent,
        picture: Option<PictureMeta>,
        crop: Option<FrameCropReport>,
    ) -> TransitionFrame {
        let wanted = self.settings.crop.wanted_zoom();
        TransitionFrame {
            event,
            plan: self.continuity.plan(now, event, wanted),
            picture,
            crop,
        }
    }

    /// Кадр без нового кадра камеры: решение с учётом того, есть ли вообще живое изображение, которое можно
    /// перекомпоновать (после неудачи сабмита или смены рисовальщика его нет).
    pub(super) fn plan_recompose_transition(&self, now: MonotonicNanos, event: StreamEvent) -> TransitionFrame {
        let mut transition = self.plan_transition(now, event, None, None);
        if !self.isp_image_shown {
            transition.plan = transition.plan.without_live_image();
        }
        transition
    }

    /// Фиксирует кадр в машине по тому, что реально записано; возвращает событие удержания. Если удержание
    /// закончилось, удержанная картинка уходит на отложенное уничтожение (после следующей записи ISP).
    pub(super) fn commit_transition(
        &mut self,
        pipelines: &mut CameraPipelines,
        now: MonotonicNanos,
        transition: &TransitionFrame,
        composed: Composed,
        processed: bool,
    ) -> crate::vulkan_backend::camera::continuity::CommitReport {
        let outcome = FrameOutcome {
            composed,
            new_picture: if processed { transition.picture } else { None },
        };
        let report = self
            .continuity
            .commit(now, transition.event, &transition.plan, &outcome);
        if matches!(report.event, Some(HoldEvent::Ended { .. })) {
            pipelines.release_held_picture();
        }
        report
    }

    /// Кадр принят очередью: счётчики удержаний, метка кадра и его кадрирование в диагностику.
    pub(super) fn note_transition_presented(
        &mut self,
        now: MonotonicNanos,
        transition: &TransitionFrame,
        report: crate::vulkan_backend::camera::continuity::CommitReport,
        new_frame_replaced_by_held: bool,
    ) {
        let capacity = self.continuity_config.recent_frames;
        let stats = self.viewfinder.continuity_mut();
        match report.event {
            Some(HoldEvent::Started(reason)) => stats.note_hold_started(reason),
            Some(HoldEvent::Ended { end, duration_ms }) => stats.note_hold_ended(end, duration_ms),
            None => {}
        }
        if new_frame_replaced_by_held {
            stats.counters_mut().frames_hidden_by_continuity += 1;
        }
        stats.note_frame(
            FrameRecord {
                present_time: now,
                label: report.label,
                backdrop: transition.plan.backdrop(),
                crop: transition.crop.clone(),
                held_magnification: transition.plan.magnification,
            },
            capacity,
        );
    }
}
