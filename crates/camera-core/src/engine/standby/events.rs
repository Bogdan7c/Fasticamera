//! События бэкенда, которые принадлежат тёплым: по поколению их узнаёт маршрутизатор
//! (`backend_events`): основная → как раньше, тёплая → сюда, иначе — устаревшее.

use super::Phase;
use crate::backend::{DeviceGeneration, SessionGeneration};
use crate::backend_standby::StandbySlotId;
use crate::engine::{CameraEngine, LOG_TARGET};
use crate::standby_status::StandbyFailureReason;

impl CameraEngine {
    /// Ячейка, чьё устройство открыто под этим поколением.
    pub(in crate::engine) fn standby_slot_of_device(&self, generation: DeviceGeneration) -> Option<StandbySlotId> {
        self.standby
            .slots
            .iter()
            .find(|slot| {
                slot.device
                    .as_ref()
                    .is_some_and(|device| device.generation == generation)
            })
            .map(|slot| slot.id)
    }

    /// Ячейка, чья сессия сконфигурирована под этим поколением.
    pub(in crate::engine) fn standby_slot_of_session(&self, generation: SessionGeneration) -> Option<StandbySlotId> {
        self.standby
            .slots
            .iter()
            .find(|slot| {
                slot.session
                    .as_ref()
                    .is_some_and(|session| session.generation == generation)
            })
            .map(|slot| slot.id)
    }

    /// Устройство или сессия тёплой потеряны (отключение, ошибка, закрытие платформой):
    /// ячейка — в `Failed`, основная не затрагивается.
    pub(in crate::engine) fn on_standby_lost(&mut self, slot: StandbySlotId, reason: StandbyFailureReason) {
        self.fail_standby_slot(slot, reason);
    }

    /// Результат кадра сессии, которая сейчас тёплая: это бывшая основная после обмена, её
    /// результаты ещё лежали в ящике. Не устаревшее событие и не событие потока — отдельный счётчик.
    pub(in crate::engine) fn note_result_after_swap(&mut self) {
        self.standby.counters.results_dropped_after_swap += 1;
    }

    /// Первый результат тёплой: она готова к повышению. `false` — сессия не ждала готовности
    /// (уже готова или это не тёплая): вызывающий считает событие устаревшим.
    pub(in crate::engine) fn on_standby_streaming(&mut self, session: SessionGeneration) -> bool {
        let Some(slot) = self.standby_slot_of_session(session) else {
            return false;
        };
        let Some(state) = self.standby.slot_mut(slot) else {
            return false;
        };
        if state.phase != Phase::Warming {
            tracing::debug!(target: LOG_TARGET, slot = slot.index(), "StandbyStreaming не для ждущей ячейки");
            return false;
        }
        state.phase = Phase::Ready;
        true
    }
}
