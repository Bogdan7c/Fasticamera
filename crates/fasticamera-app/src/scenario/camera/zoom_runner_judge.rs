//! Суждения исполнителя `zoom` о шаге: почему вышел срок и что видно на экране в конце шага. Дочерний модуль
//! `zoom_runner.rs` (размер файла), поведение не менялось.

use super::{ActiveStep, SCREEN_RATIO_TOLERANCE_MILLI, ZoomObservation, ZoomRunner, ZoomStepRecord, expectation_name};

impl ZoomRunner {
    pub(super) fn timeout_reason(&self, active: &ActiveStep) -> String {
        match (&active.switch, active.expectation) {
            (None, expectation) => format!(
                "за {} мс движок не сообщил о смене объектива ({})",
                self.config.step_timeout_ms,
                expectation_name(expectation)
            ),
            (Some(_), _) => format!(
                "смена объектива сообщена, но живого кадра новой камеры на экране нет за {} мс",
                self.config.step_timeout_ms
            ),
        }
    }

    /// Проверки состояния экрана в конце шага (то, что видно пользователю): показанный коэффициент.
    pub(super) fn judge_screen(
        &self,
        active: &ActiveStep,
        record: &ZoomStepRecord,
        observation: &ZoomObservation<'_>,
    ) -> Option<String> {
        if let Some(target) = active.target_ratio_milli {
            let shown = observation.screen.zoom.ratio_milli;
            if shown.abs_diff(target) > SCREEN_RATIO_TOLERANCE_MILLI {
                return Some(format!("экран показывает {shown} тыс., а выбран упор {target} тыс."));
            }
        }
        let moves_zoom = active.expectation.expects_switch() || active.drag.is_some();
        if moves_zoom && record.lens_intents_published == 0 {
            return Some("экран не положил намерение объектива в ящик движка".to_owned());
        }
        if active.expectation.expects_switch() && active.new_camera.is_none() {
            return Some("смена объектива есть, а камеру новой роли в каталоге зума найти не удалось".to_owned());
        }
        record.failure.clone()
    }
}
