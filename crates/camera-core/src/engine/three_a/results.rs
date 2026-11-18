//! Результаты кадров, таймеры и жизненный цикл потока для 3A.
//!
//! Результат кадра попадает: в «последний результат» снимка (только если кадр новее — разовые
//! результаты идут по невытесняемой полосе ящика и могут обгонять старые результаты потока,
//! поэтому порядок определяет `frame_number`, а не приход), исполнителям, и, если это кадр
//! серии, в сбор серии.

use std::time::{Duration, Instant};

use camera_contract::capture_result::CaptureResultMeta;
use camera_contract::three_a::RequestOrigin;

use super::super::CameraEngine;
use crate::event::CameraEvent;
use crate::three_a_control::PlanRefresh;
use crate::three_a_status::{BracketFailure, LastThreeAResult};

impl CameraEngine {
    /// Результат кадра текущей сессии.
    pub(in crate::engine) fn observe_three_a_result(&mut self, result: &CaptureResultMeta) {
        let origin = result.three_a.origin;
        if origin == RequestOrigin::Unknown {
            self.three_a.counters.results_with_unknown_origin += 1;
        }
        self.record_last_three_a_result(result);
        self.three_a.exposure.observe_result(result);
        if let Some((request, outcome)) = self.three_a.focus.observe_result(result) {
            if self
                .three_a
                .focus_settle_deadline
                .is_some_and(|(waited, _)| waited == request)
            {
                self.three_a.focus_settle_deadline = None;
            }
            self.events.push(CameraEvent::FocusSettled { request, outcome });
        }
        if let RequestOrigin::Burst { burst, index } = origin {
            self.collect_bracket_frame(burst, index, result);
        }
    }

    fn record_last_three_a_result(&mut self, result: &CaptureResultMeta) {
        let is_newer = self
            .three_a
            .last_result
            .is_none_or(|last| result.frame_number >= last.frame_number);
        if !is_newer {
            return;
        }
        self.three_a.last_result = Some(LastThreeAResult {
            frame_number: result.frame_number,
            sensor_timestamp: result.sensor_timestamp,
            origin: result.three_a.origin,
            af_state: result.three_a.af_state,
            ae_state: result.three_a.ae_state,
            focus_distance_diopters: result.three_a.focus_distance_diopters,
            exposure_time: result.exposure_time,
            sensitivity_iso: result.sensitivity_iso,
        });
    }

    /// Исполнители получили решение со стороны (пробуждение [`crate::ThreeAWaker`]): наш AE
    /// (08a) или трекер фокуса (задел, решение 69). Спрашиваем **обоих** (без короткого
    /// замыкания: каждый должен забрать своё новое решение), запрос перестраивается один раз
    /// и только если хоть один план действительно изменился.
    pub fn refresh_three_a(&mut self, now: Instant) {
        if self.poisoned {
            return;
        }
        let exposure = self.three_a.exposure.refresh();
        let focus = self.three_a.focus.refresh();
        if exposure == PlanRefresh::Changed || focus == PlanRefresh::Changed {
            self.reconcile(now);
        }
    }

    /// Сработали таймеры 3A: итог тапа и результаты серии.
    pub(in crate::engine) fn fire_three_a_timers(&mut self, now: Instant) {
        if let Some((request, _)) = self.three_a.focus_settle_deadline.filter(|(_, due)| now >= *due) {
            self.three_a.focus_settle_deadline = None;
            if let Some(outcome) = self.three_a.focus.settle_timed_out(request) {
                self.three_a.counters.focus_timeouts += 1;
                self.events.push(CameraEvent::FocusSettled { request, outcome });
            }
        }
        if self
            .three_a
            .bracket
            .as_ref()
            .and_then(|run| run.deadline())
            .is_some_and(|due| now >= due)
        {
            self.fail_bracket(BracketFailure::ResultsTimedOut);
        }
    }

    /// Поток закончился (сессия закрыта, камера потеряна, запрос остановлен): всё, что
    /// относилось к нему, забыто. Исполнители сбрасываются, поэтому тап, оставшийся в
    /// намерении, после восстановления запустится заново.
    pub(in crate::engine) fn end_three_a_stream(&mut self) {
        self.fail_bracket(BracketFailure::SessionLost);
        self.three_a.focus_settle_deadline = None;
        self.three_a.announced_adjustments.clear();
        self.three_a.focus.reset_for_new_stream();
        self.three_a.exposure.reset_for_new_stream();
    }

    /// Срок ожидания результатов серии из конфига.
    pub(super) fn bracket_result_timeout(&self) -> Duration {
        Duration::from_millis(self.config.three_a.bracket_result_timeout_ms)
    }
}
