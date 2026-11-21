//! Повышение тёплой до основной: второй способ исполнить смену объектива.
//!
//! Смена как состояние (`LensSwitchState::Switching`) одна. Её исполняют двумя способами:
//! переоткрытием (`step_release_wrong_device` → `open` → `configure`) или, если желаемая камера
//! уже стримит тёплой и готова, обменом ролей без закрытия и открытия. Решение «что исполнять»
//! принимает `reconcile` (не команда: команда только меняет желаемое и отвечает), поэтому
//! повышение стоит первым шагом `drive_to_plan`.
//!
//! Отказ бэкенда в обмене ничего не меняет: движок идёт обычным путём.

use camera_contract::zoom::LensZoomPoint;

use super::Phase;
use crate::backend_standby::{StandbySlotId, StandbySupport};
use crate::engine::reconcile::Plan;
use crate::engine::{CameraEngine, LOG_TARGET};
use crate::zoom::SwitchCost;

impl CameraEngine {
    /// Если желаемая камера — готовая тёплая, обменять её с основной. Иначе — ничего не делать.
    pub(in crate::engine) fn step_promote_standby(&mut self, plan: &Plan) {
        let Some(slot) = self.promotable_slot(plan) else {
            return;
        };
        // Поддержку проверяем ДО объявления: иначе ожидаемая экспозиция осталась бы объявленной
        // без обмена (ревью 13b, замечание 4).
        if matches!(self.backend.standby(), StandbySupport::Unsupported) {
            return;
        }
        // Рендер должен знать ожидаемую экспозицию новой основной к её первому кадру: объявляем до обмена.
        self.announce_expected_exposure_before_swap(&plan.camera);
        let outcome = match self.backend.standby() {
            StandbySupport::Available(backend) => backend.swap_standby_with_primary(slot),
            // Недостижимо: проверено выше, а бэкенд между вызовами не меняется (один поток камеры).
            StandbySupport::Unsupported => {
                self.withdraw_expected_exposure();
                return;
            }
        };
        match outcome {
            Ok(()) => self.complete_promotion(slot, plan),
            Err(failure) => {
                // Ничего не изменено ни у бэкенда, ни у нас: ниже пойдёт обычное переоткрытие.
                tracing::warn!(target: LOG_TARGET, %failure, "обмен ролей отвергнут: переоткрываем камеру");
                self.standby.counters.promotion_fallbacks += 1;
                self.withdraw_expected_exposure();
            }
        }
    }

    /// Ячейка, которую можно повысить для плана `plan`: основная стримит другую камеру, а у
    /// ячейки желаемой камеры готовый поток ровно того режима, который хочет план.
    fn promotable_slot(&self, plan: &Plan) -> Option<StandbySlotId> {
        let primary = self.device.as_ref()?;
        if primary.camera == plan.camera || !self.primary_is_streaming() {
            return None;
        }
        let slot = self.standby.ready_slot_for(&plan.camera)?;
        let configured = self.standby.slots.get(slot.index())?.session.as_ref()?;
        (plan.stream.as_ref() == Some(&configured.configured.stream)).then_some(slot)
    }

    /// Сколько стоит перейти на объектив `lens` прямо сейчас: обмен ролей, если его камера уже
    /// готова тёплой при стримящей основной, иначе переоткрытие. Селектор по этому ответу решает,
    /// нужен ли гистерезис (тёплая камера включается ровно на своём коэффициенте).
    pub(in crate::engine) fn switch_cost_to(&self, lens: &LensZoomPoint) -> SwitchCost {
        let is_primary = self
            .device
            .as_ref()
            .is_some_and(|primary| primary.camera == lens.camera);
        if !is_primary && self.primary_is_streaming() && self.standby.ready_slot_for(&lens.camera).is_some() {
            SwitchCost::CheapPromotion
        } else {
            SwitchCost::ExpensiveReopen
        }
    }

    /// Бэкенд поменял роли: поменять их и в движке, сбросить всё, что принадлежало прошлому потоку.
    fn complete_promotion(&mut self, slot: StandbySlotId, plan: &Plan) {
        let Some(state) = self.standby.slot_mut(slot) else {
            return;
        };
        // Обмен: тёплая становится основной, прежняя основная остаётся в той же ячейке и стримит.
        std::mem::swap(&mut self.device, &mut state.device);
        std::mem::swap(&mut self.session, &mut state.session);
        state.camera = state.device.as_ref().map(|device| device.camera.clone());
        // Прежняя основная уже отдавала кадры: готова сразу (`StandbyStreaming` для неё не придёт).
        // Запрос у неё пока основной; тёплый перепоставит `sync_standby`.
        state.phase = Phase::Ready;
        state.issued = None;
        self.standby.counters.promotions += 1;

        // Новая основная: поток, 3A и темп начинаются заново, как при обычной смене камеры.
        self.end_three_a_stream();
        self.carry_exposure_memory_to(&plan.camera);
        if let Some(session) = &mut self.session {
            // Запрос новой основной перепоставит `step_repeating_request` по плану 3A этой камеры.
            session.repeating = None;
            if let Some(stream) = &plan.stream {
                self.cadence
                    .begin_session(session.generation, stream.frame_rate.frame_duration());
            }
        }
        self.note_lens_promoted_standby();
        tracing::info!(target: LOG_TARGET, camera = %plan.camera, "тёплая камера повышена до основной");
    }
}
