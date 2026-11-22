//! Fake для вспышки (14d): сценарий отказов разового запроса и выдача его результата.

use camera_contract::flash::{FlashResult, FlashShotId};
use camera_contract::three_a::RequestOrigin;

use super::{FakeBackendControl, FakeEventDelivery};
use crate::backend::{BackendEvent, RequestFailure};
use crate::request_controls::RequestControls;

impl FakeBackendControl {
    /// Следующий `submit_flash_still` вернёт этот итог (очередь; пустая — успех).
    pub fn push_flash_still_result(&self, result: Result<(), RequestFailure>) {
        self.lock().flash_script.push_back(result);
    }

    /// Контролы последнего принятого разового запроса со вспышкой.
    pub fn last_submitted_flash_still(&self) -> Option<(FlashShotId, RequestControls)> {
        self.lock().submitted_flash_stills.last().copied()
    }

    /// Результат кадра разового запроса `shot` со `flash` в нём. Выдержка и ISO — те, что отправлены.
    /// Запрос, которого бэкенд не принимал, — `NothingToAttachTo`.
    pub fn complete_flash_still(&self, shot: FlashShotId, flash: FlashResult) -> FakeEventDelivery {
        let Some(session) = self.current_session_generation() else {
            return FakeEventDelivery::NothingToAttachTo;
        };
        if !self
            .lock()
            .submitted_flash_stills
            .iter()
            .any(|(submitted, _)| *submitted == shot)
        {
            return FakeEventDelivery::NothingToAttachTo;
        }
        let mut result = self.next_result(RequestOrigin::FlashStill { shot }, super::three_a::FAKE_FRAME_INTERVAL);
        result.three_a.flash = flash;
        self.send_event(BackendEvent::CaptureCompleted { session, result })
    }

    /// Приём разового запроса бэкендом: сценарий, потом проверка сессии; принятый запоминается.
    pub(super) fn accept_flash_still(
        &self,
        controls: &RequestControls,
        shot: FlashShotId,
    ) -> Result<(), RequestFailure> {
        let mut state = self.lock();
        let outcome = match state.flash_script.pop_front() {
            Some(scripted) => scripted,
            None if state.session.is_none() => Err(RequestFailure::NoSession),
            None => Ok(()),
        };
        if outcome.is_ok() {
            state.submitted_flash_stills.push((shot, *controls));
        }
        outcome
    }
}
