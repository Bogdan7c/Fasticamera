//! Память о доступности камер: нужна, чтобы не терять сигнал «камера свободна», пришедший
//! раньше, чем движок успел начать ждать.
//!
//! Гонка: `onCameraAvailable` приходит на потоке обработчика доступности NDK, а
//! `DeviceDisconnected` — на looper-е устройства. Это разные потоки, и «камера свободна» может
//! лечь в ящик **раньше** отключения, из-за которого мы только собираемся ждать. Без памяти
//! такой сигнал игнорируется (ждать ещё нечего), и ожидание становится вечным.
//!
//! Порядок событий определяется **номером события**: движок нумерует каждое событие бэкенда
//! при разборе (одним потоком, поэтому нумерация строгая). Попытка открытия запоминает номер,
//! при котором она сделана. «Свободна» считается новой, только если пришла **позже** последней
//! попытки открытия: сигнал «свободна» от времени до нашего открытия (в том числе вызванный
//! нашим же закрытием предыдущего устройства) не должен запускать повтор.

use std::collections::HashMap;

use camera_contract::ids::CameraId;

use crate::backend::CameraAvailability;

/// Последняя известная доступность камер и порядок событий.
#[derive(Debug, Default)]
pub(super) struct AvailabilityMemory {
    /// Номер последнего разобранного события бэкенда.
    event_number: u64,
    /// Номер события, при котором в последний раз вызывался `open_device`.
    last_open_attempt: u64,
    /// Для каждой камеры: что известно и при каком событии.
    known: HashMap<CameraId, (CameraAvailability, u64)>,
}

impl AvailabilityMemory {
    /// Начался разбор очередного события бэкенда (до его обработки).
    pub(super) fn begin_event(&mut self) {
        self.event_number += 1;
    }

    /// Запомнить сообщённую доступность камеры (вызывать после [`Self::begin_event`]).
    pub(super) fn record(&mut self, camera: &CameraId, availability: CameraAvailability) {
        self.known.insert(camera.clone(), (availability, self.event_number));
    }

    /// Движок вызывает `open_device`: всё, что было известно до этого момента, устарело.
    pub(super) fn note_open_attempt(&mut self) {
        self.last_open_attempt = self.event_number;
    }

    /// Камера освободилась уже **после** нашей последней попытки её открыть.
    pub(super) fn freed_since_last_open_attempt(&self, camera: &CameraId) -> bool {
        self.known.get(camera).is_some_and(|(availability, at)| {
            *availability == CameraAvailability::Available && *at > self.last_open_attempt
        })
    }
}

#[cfg(test)]
mod tests {
    use camera_contract::fixtures::camera_id;

    use super::*;

    #[test]
    fn unknown_camera_is_not_considered_freed() {
        let memory = AvailabilityMemory::default();
        assert!(!memory.freed_since_last_open_attempt(&camera_id("2")));
    }

    #[test]
    fn available_after_the_open_attempt_counts_but_before_it_does_not() {
        let mut memory = AvailabilityMemory::default();
        memory.begin_event();
        memory.record(&camera_id("2"), CameraAvailability::Available);
        memory.begin_event();
        memory.note_open_attempt();
        assert!(
            !memory.freed_since_last_open_attempt(&camera_id("2")),
            "свободна была до открытия"
        );

        memory.begin_event();
        memory.record(&camera_id("2"), CameraAvailability::Available);
        assert!(memory.freed_since_last_open_attempt(&camera_id("2")));
        assert!(
            !memory.freed_since_last_open_attempt(&camera_id("3")),
            "другая камера не в счёт"
        );
    }

    #[test]
    fn later_unavailable_overrides_earlier_available() {
        let mut memory = AvailabilityMemory::default();
        memory.begin_event();
        memory.record(&camera_id("2"), CameraAvailability::Available);
        memory.begin_event();
        memory.record(&camera_id("2"), CameraAvailability::Unavailable);
        assert!(!memory.freed_since_last_open_attempt(&camera_id("2")));
    }

    #[test]
    fn available_in_the_same_event_as_the_open_attempt_is_not_new() {
        // Повтор запущен самим событием «свободна»: открытие делается при том же номере.
        let mut memory = AvailabilityMemory::default();
        memory.begin_event();
        memory.record(&camera_id("2"), CameraAvailability::Available);
        memory.note_open_attempt();
        assert!(!memory.freed_since_last_open_attempt(&camera_id("2")));
    }
}
