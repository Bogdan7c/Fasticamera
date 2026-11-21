//! Бесшовные шаги в исполнителе `zoom` (13b): мост между [`ZoomRunner`] и машиной жеста
//! ([`super::super::zoom_seamless`]). Живёт дочерним модулем исполнителя, чтобы видеть его закрытые поля и не
//! раздувать `zoom_runner.rs`.

use super::{ActiveStep, Injection, ZoomObservation, ZoomRunner};
use crate::scenario::camera::zoom_plan::{SeamlessGesture, SeamlessLeg, SwitchExpectation};
use crate::scenario::camera::zoom_seamless::{self, SeamlessProgress, SeamlessSignal};

impl ZoomRunner {
    /// Начать часть бесшовного жеста: ожидание объектива, первые касания и сама машина жеста.
    pub(super) fn begin_seamless(
        &mut self,
        observation: &ZoomObservation<'_>,
        gesture: SeamlessGesture,
        leg: SeamlessLeg,
    ) -> Result<(SwitchExpectation, Vec<Injection>, SeamlessProgress), String> {
        let start = zoom_seamless::begin(gesture, leg, &self.config, observation, &mut self.held)?;
        Ok((start.expectation, start.injections, start.progress))
    }

    /// Один тик машины жеста. `Some(причина)` — часть жеста не удалась.
    pub(super) fn drive_seamless(
        &mut self,
        active: &mut ActiveStep,
        observation: &ZoomObservation<'_>,
        out: &mut Vec<Injection>,
    ) -> Option<String> {
        let progress = active.seamless.as_mut()?;
        let tick = progress.tick(observation);
        out.extend(tick.injections);
        match tick.signal {
            SeamlessSignal::None => None,
            SeamlessSignal::CrossingStarted => {
                // Подготовка (нажатие, ожидание тёплой, окно частоты) закончена: времена шага («действие → смена
                // объектива», «действие → первый живой кадр») считаются от начала движения через порог.
                active.started_at = observation.now;
                active.action_render = observation.render_now;
                self.ring.reset_overrun();
                None
            }
            SeamlessSignal::Failed(reason) => Some(reason),
        }
    }

    /// Пальцы, которые часть жеста оставляет лежать на экране, переходят исполнителю (до следующей части).
    pub(super) fn stash_fingers(&mut self, active: &mut ActiveStep) {
        self.held = active.seamless.take().and_then(SeamlessProgress::into_held);
    }

    /// Поднять пальцы, оставшиеся на экране после провала.
    pub(super) fn lift_stranded_fingers(&mut self, observation: &ZoomObservation<'_>) -> Vec<Injection> {
        let Some(mut fingers) = self.held.take() else {
            return Vec::new();
        };
        fingers
            .lift(observation.render_now)
            .into_iter()
            .map(Injection::Touch)
            .collect()
    }
}
