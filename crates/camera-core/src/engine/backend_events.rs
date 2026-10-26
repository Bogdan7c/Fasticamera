//! События бэкенда: проверка поколений, темп потока и реакция на отказы устройства.
//!
//! Событие, чьё поколение не совпадает с текущими устройством или сессией, — «привидение» от
//! уже закрытого: оно отбрасывается и считается в `stale_backend_events`. Так запоздалое
//! `DeviceDisconnected` старого устройства не убьёт переоткрытое.

use std::time::Instant;

use camera_contract::availability::CameraUnavailableReason;

use super::{CameraEngine, LOG_TARGET};
use crate::backend::{BackendEvent, DeviceGeneration, SessionGeneration};

impl CameraEngine {
    fn is_current_device(&self, generation: DeviceGeneration) -> bool {
        self.device
            .as_ref()
            .is_some_and(|device| device.generation == generation)
    }

    fn is_current_session(&self, generation: SessionGeneration) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.generation == generation)
    }

    fn discard_stale_event(&mut self, event: &BackendEvent) {
        self.counters.stale_backend_events += 1;
        tracing::debug!(target: LOG_TARGET, ?event, "событие бэкенда прошлого устройства или сессии отброшено");
    }

    pub(super) fn route_backend_event(&mut self, event: BackendEvent, now: Instant) {
        self.availability.begin_event();
        match event {
            BackendEvent::DeviceDisconnected { device } => {
                if self.is_current_device(device) {
                    let _ = self.fail_recoverable(CameraUnavailableReason::Evicted, now);
                    self.retry_now_if_camera_already_freed(now);
                } else {
                    self.discard_stale_event(&event);
                }
            }
            BackendEvent::DeviceError { device, error } => {
                if self.is_current_device(device) {
                    self.fail_from_device_error(error, now);
                    self.retry_now_if_camera_already_freed(now);
                } else {
                    self.discard_stale_event(&event);
                }
            }
            BackendEvent::SessionClosed { session } => {
                if self.is_current_session(session) {
                    self.handle_unexpected_session_close(now);
                } else {
                    self.discard_stale_event(&event);
                }
            }
            BackendEvent::CaptureCompleted { session, ref result } => {
                if self.is_current_session(session) {
                    self.cadence.record_completed(result);
                    self.note_stable_frame();
                } else {
                    self.discard_stale_event(&event);
                }
            }
            BackendEvent::CaptureFailed { session, reason, .. } => {
                if self.is_current_session(session) {
                    self.cadence.record_failed(reason);
                } else {
                    self.discard_stale_event(&event);
                }
            }
            BackendEvent::CameraAvailability {
                ref camera,
                availability,
            } => {
                self.on_camera_availability(camera, availability, now);
            }
        }
    }

    /// Платформа закрыла текущую сессию, а движок её не закрывал (иначе поколение уже сброшено).
    ///
    /// Это признак проблемы устройства, поэтому путь тот же, что у сбоя устройства
    /// (`DeviceFailure`): устройство закрывается, повтор идёт по таймеру с нарастающей задержкой
    /// и упирается в лимит подряд идущих попыток. Мгновенный пересоздающий повтор был бы
    /// бесконечным циклом без паузы, если платформа закрывает каждую новую сессию.
    fn handle_unexpected_session_close(&mut self, now: Instant) {
        tracing::warn!(target: LOG_TARGET, "платформа закрыла сессию, которую движок не закрывал");
        let _ = self.fail_recoverable(CameraUnavailableReason::DeviceFailure, now);
    }
}
