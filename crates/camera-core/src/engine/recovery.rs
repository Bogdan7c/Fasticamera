//! Недоступность камеры и восстановление (решение владельца 39).
//!
//! Любая недоступность (вытеснение, занятость, сбой HAL, запрет) проходит один путь
//! [`CameraEngine::fail_recoverable`]: устройство закрывается на бэкенде, публикуется
//! `Unavailable{reason}`, состояние становится `Error{Recoverable{reason, plan}}`. План
//! зависит от причины:
//! * вытеснение / занята / слишком много камер / камеры нет → ждём сигнала «камера свободна»;
//! * сбой устройства или службы → повтор по таймеру с нарастающей задержкой;
//! * нет разрешения / запрет политикой → ждём возврата на передний план.
//!
//! Попытка — это запуск повтора ([`RecoveryTracker::start_attempt`]). Если после `max`
//! подряд идущих попыток снова ошибка — `Fatal::RecoveryExhausted`. Первая ошибка рабочего
//! потока попыткой не считается. Возврат в целевое состояние публикует `Recovered`, но счётчик
//! подряд обнуляется только после **устойчивой** работы: `stable_frames_before_reset` результатов
//! кадров подряд от текущей сессии (или сразу, если поток не желаем и ждать кадров нечего).
//! Иначе «настроили → платформа закрыла сессию → настроили…» каждый раз начинался бы с попытки 1.

use std::time::{Duration, Instant};

use camera_contract::availability::CameraUnavailableReason;

use super::lens_switch::is_fault_of_new_camera;
use super::{CameraEngine, Halt, LOG_TARGET};
use crate::backend::{BackendFailure, CameraAvailability, DeviceErrorKind};
use crate::event::CameraEvent;
use crate::lens_status::LensSwitchFailure;
use crate::state::{CameraError, CameraState, FatalCause, RecoveryPlan};
use crate::status::{AppPresence, CameraCounters};

/// Учёт восстановления: счётчик попыток, невыполненная причина и ожидание повтора.
#[derive(Debug, Default)]
pub(super) struct RecoveryTracker {
    /// Начатые подряд попытки без успеха.
    pub(super) consecutive_attempts: u32,
    /// Причина, по которой ещё не выдано `Recovered`.
    pub(super) unresolved: Option<CameraUnavailableReason>,
    /// Движок в `Error::Recoverable` и ждёт повода повторить: доведение до желаемого заперто.
    pub(super) waiting: bool,
    pub(super) retry_deadline: Option<Instant>,
    /// Восстановились, но счётчик попыток ещё не обнулён: сколько результатов кадров подряд
    /// отдала сессия после этого. `None` — наблюдения нет (счётчик уже обнулён или не нужен).
    pub(super) stability_frames_seen: Option<u32>,
}

impl RecoveryTracker {
    /// Начать попытку: учесть её и снять запрет на доведение.
    pub(super) fn start_attempt(&mut self, counters: &mut CameraCounters) {
        self.consecutive_attempts += 1;
        counters.recovery_attempts += 1;
        self.stop_waiting();
    }

    /// Перестать ждать повода (повтор начат, камера закрыта или приложение ушло в фон).
    pub(super) fn stop_waiting(&mut self) {
        self.waiting = false;
        self.retry_deadline = None;
    }

    /// Счётчик подряд идущих попыток с нуля (возврат на передний план).
    pub(super) fn reset_attempts(&mut self) {
        self.consecutive_attempts = 0;
        self.stability_frames_seen = None;
    }

    /// Результат кадра текущей сессии. Возвращает `true`, если набралось `required` и счётчик
    /// попыток обнулён.
    pub(super) fn note_frame(&mut self, required: u32) -> bool {
        let Some(seen) = self.stability_frames_seen else {
            return false;
        };
        let seen = seen.saturating_add(1);
        if seen >= required {
            self.reset_attempts();
            return true;
        }
        self.stability_frames_seen = Some(seen);
        false
    }

    /// Забыть всё (камера закрыта по команде или выбрана другая).
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Причина недоступности по коду ошибки устройства.
fn reason_for_device_error(error: DeviceErrorKind) -> CameraUnavailableReason {
    match error {
        DeviceErrorKind::CameraInUse => CameraUnavailableReason::InUseByAnotherClient,
        DeviceErrorKind::MaxCamerasInUse => CameraUnavailableReason::TooManyCamerasOpen,
        DeviceErrorKind::CameraDisabled => CameraUnavailableReason::DisabledByPolicy,
        DeviceErrorKind::CameraDevice | DeviceErrorKind::Unknown(_) => CameraUnavailableReason::DeviceFailure,
        DeviceErrorKind::CameraService => CameraUnavailableReason::ServiceFailure,
    }
}

impl CameraEngine {
    /// Причина недоступности для ошибки устройства (используют `backend_events` и `reconcile`).
    pub(super) fn unavailable_reason_for(error: DeviceErrorKind) -> CameraUnavailableReason {
        reason_for_device_error(error)
    }

    /// План выхода из ошибки для причины; `attempt` — номер ближайшей попытки.
    fn plan_for(&self, reason: CameraUnavailableReason, attempt: u32) -> RecoveryPlan {
        match reason {
            CameraUnavailableReason::Evicted
            | CameraUnavailableReason::InUseByAnotherClient
            | CameraUnavailableReason::CameraNotPresent => RecoveryPlan::WaitForCameraAvailable,
            // Лимит камер общий: доступность желаемой камеры не меняется, сигнала «свободна»
            // не будет, поэтому ждать его вечно нельзя — повтор по таймеру, как при сбое.
            CameraUnavailableReason::TooManyCamerasOpen
            | CameraUnavailableReason::DeviceFailure
            | CameraUnavailableReason::ServiceFailure => RecoveryPlan::RetryAfterDelay {
                attempt,
                delay_ms: self.config.backoff_for_attempt(attempt),
            },
            CameraUnavailableReason::PermissionDenied | CameraUnavailableReason::DisabledByPolicy => {
                RecoveryPlan::WaitForExternalAction
            }
        }
    }

    /// Камера пропала: закрыть устройство, объявить причину и выбрать план (или сдаться).
    ///
    /// Возвращает [`Halt`], чтобы вызывающий шаг доведения мог остановиться одним `return Err(..)`.
    pub(super) fn fail_recoverable(&mut self, reason: CameraUnavailableReason, now: Instant) -> Halt {
        // Посреди смены объектива отказ самой новой камеры — не повод ждать её: возвращаемся.
        if is_fault_of_new_camera(reason)
            && let Some(halt) = self.try_revert_lens_switch(LensSwitchFailure::Unavailable { reason })
        {
            return halt;
        }
        tracing::warn!(target: LOG_TARGET, reason = reason.name(), "камера недоступна");
        self.release_platform();
        self.reconfiguring = false;
        self.last_unavailable = Some(reason);
        self.events.push(CameraEvent::Unavailable { reason });
        self.recovery.unresolved = Some(reason);
        // Новый отказ раньше устойчивости: счётчик попыток сохраняется, наблюдение начнётся заново.
        self.recovery.stability_frames_seen = None;

        let attempts = self.recovery.consecutive_attempts;
        if attempts >= self.config.recovery.max_consecutive_attempts {
            self.enter_fatal(FatalCause::RecoveryExhausted { reason, attempts });
            return Halt;
        }
        let plan = self.plan_for(reason, attempts + 1);
        self.recovery.waiting = true;
        self.recovery.retry_deadline = match plan {
            RecoveryPlan::RetryAfterDelay { delay_ms, .. } => Some(now + Duration::from_millis(delay_ms)),
            RecoveryPlan::WaitForCameraAvailable | RecoveryPlan::WaitForExternalAction => None,
        };
        // В фоне ошибка только фиксируется: состояние остаётся «закрыто, приложение в фоне».
        if self.presence == AppPresence::Foreground {
            self.transition(CameraState::Error {
                error: CameraError::Recoverable { reason, recovery: plan },
            });
        } else {
            self.recovery.stop_waiting();
        }
        Halt
    }

    /// Сбой платформы без смысловой причины (код и контекст в лог) — как сбой устройства.
    pub(super) fn fail_with_backend_failure(&mut self, failure: &BackendFailure, now: Instant) -> Halt {
        tracing::error!(target: LOG_TARGET, %failure, "вызов бэкенда камеры завершился отказом");
        self.fail_recoverable(CameraUnavailableReason::DeviceFailure, now)
    }

    /// Камера освободилась (или заняли): повтор, если ждали именно её.
    pub(super) fn on_camera_availability(
        &mut self,
        camera: &camera_contract::ids::CameraId,
        availability: CameraAvailability,
        now: Instant,
    ) {
        // Запоминаем всегда: «свободна» может прийти раньше, чем мы начнём ждать (гонка потоков NDK).
        self.availability.record(camera, availability);
        if availability != CameraAvailability::Available || !self.recovery.waiting {
            return;
        }
        let waiting_for_availability = matches!(
            self.state,
            CameraState::Error {
                error: CameraError::Recoverable {
                    recovery: RecoveryPlan::WaitForCameraAvailable,
                    ..
                }
            }
        );
        let is_wanted_camera = self.wanted_camera_id().as_ref() == Some(camera);
        if waiting_for_availability && is_wanted_camera && self.presence == AppPresence::Foreground {
            self.recovery.start_attempt(&mut self.counters);
            self.reconcile(now);
        }
    }

    /// Идентификатор желаемой камеры по текущему каталогу.
    fn wanted_camera_id(&self) -> Option<camera_contract::ids::CameraId> {
        let target = self.desired.target.as_ref()?;
        self.catalog
            .resolve(target)
            .ok()
            .map(|descriptor| descriptor.id.clone())
    }

    /// Мы только что начали ждать «камера свободна», а сигнал уже приходил (раньше, чем
    /// отключение, из-за которого ждём): повторить сразу, а не ждать вечно.
    ///
    /// «Приходил» — строго после последней нашей попытки открыть камеру, см. `availability_memory`.
    pub(super) fn retry_now_if_camera_already_freed(&mut self, now: Instant) {
        let waiting_for_availability = self.recovery.waiting
            && matches!(
                self.state,
                CameraState::Error {
                    error: CameraError::Recoverable {
                        recovery: RecoveryPlan::WaitForCameraAvailable,
                        ..
                    }
                }
            );
        if !waiting_for_availability || self.presence != AppPresence::Foreground {
            return;
        }
        let Some(camera) = self.wanted_camera_id() else {
            return;
        };
        if self.availability.freed_since_last_open_attempt(&camera) {
            tracing::info!(target: LOG_TARGET, "камера освободилась до начала ожидания: повтор сразу");
            self.recovery.start_attempt(&mut self.counters);
            self.reconcile(now);
        }
    }

    /// Результат кадра текущей сессии: копим устойчивость и, набрав порог, обнуляем счётчик попыток.
    pub(super) fn note_stable_frame(&mut self) {
        let required = self.config.recovery.stable_frames_before_reset;
        if self.recovery.note_frame(required) {
            tracing::info!(target: LOG_TARGET, frames = required, "поток устойчив: счётчик попыток восстановления обнулён");
        }
    }

    /// Момент следующего повтора по таймеру.
    pub(super) fn pending_retry_deadline(&self) -> Option<Instant> {
        if self.recovery.waiting && self.presence == AppPresence::Foreground {
            self.recovery.retry_deadline
        } else {
            None
        }
    }

    pub(super) fn fire_retry_timer_if_due(&mut self, now: Instant) {
        let Some(deadline) = self.pending_retry_deadline() else {
            return;
        };
        if now < deadline {
            return;
        }
        self.recovery.start_attempt(&mut self.counters);
        self.reconcile(now);
    }

    /// После успешного доведения: если шли из недоступности — объявить `Recovered`.
    pub(super) fn complete_recovery_if_pending(&mut self) {
        let Some(reason) = self.recovery.unresolved.take() else {
            return;
        };
        let attempts = self.recovery.consecutive_attempts;
        // Счётчик попыток здесь НЕ обнуляется (платформа может тут же снова оборвать сессию).
        // Кадры ждём, только если поток действительно идёт; иначе дожидаться нечего.
        if self.session.as_ref().is_some_and(|session| session.repeating.is_some()) {
            self.recovery.stability_frames_seen = Some(0);
        } else {
            self.recovery.reset_attempts();
        }
        self.counters.recoveries += 1;
        tracing::info!(target: LOG_TARGET, reason = reason.name(), attempts, "камера восстановлена");
        self.events.push(CameraEvent::Recovered { reason, attempts });
    }
}
