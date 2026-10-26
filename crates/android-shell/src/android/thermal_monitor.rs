//! Тепловой монитор: подписка на смену статуса + периодический опрос запаса.
//!
//! Работает весь процесс в своём потоке. Каждое показание уходит событием
//! `ShellEvent::ThermalStatus` — решения принимает `ThermalPolicy`, а не монитор.
//! Поток будят либо смена статуса (колбэк NDK), либо таймер опроса.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use shell_contract::event::ShellEvent;
use shell_contract::event_queue::{SendOutcome, ShellEventSender};
use shell_contract::thermal::{HeadroomForecast, ThermalReading, ThermalSource};

use super::thermal_ffi::{StatusListener, ThermalManager};

/// Состояние подписки на смену статуса (для отчётов).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ThermalListenerState {
    Registered,
    /// Регистрация отвергнута (errno из NDK); монитор живёт только опросом.
    RegistrationFailed {
        status: i32,
    },
    /// Теплового сервиса нет — монитор не запущен.
    ManagerUnavailable {
        reason: String,
    },
    /// Сервис есть, но поток монитора не создан: показаний не будет, подписка снята.
    MonitorThreadFailed {
        reason: String,
    },
}

/// Работающий монитор.
#[derive(Debug)]
pub struct ThermalMonitor {
    manager: Option<Arc<ThermalManager>>,
    listener_state: ThermalListenerState,
}

impl ThermalMonitor {
    /// Источник показаний для прямых вопросов (`None`, если сервиса нет).
    pub fn source(&self) -> Option<&Arc<ThermalManager>> {
        self.manager.as_ref()
    }

    pub fn listener_state(&self) -> &ThermalListenerState {
        &self.listener_state
    }
}

/// Запускает монитор. Отказ сервиса — не ошибка запуска оболочки: он виден в состоянии.
pub fn start_thermal_monitor(poll_interval: Duration, events: ShellEventSender) -> ThermalMonitor {
    let manager = match ThermalManager::acquire() {
        Ok(manager) => Arc::new(manager),
        Err(error) => {
            tracing::error!(target: "fasticamera::shell", %error, "тепловой монитор не запущен");
            return ThermalMonitor {
                manager: None,
                listener_state: ThermalListenerState::ManagerUnavailable {
                    reason: error.to_string(),
                },
            };
        }
    };
    // Одно место: несколько смен статуса подряд сливаются в одно пробуждение.
    let (wake_sender, wake_receiver) = sync_channel(1);
    let (listener, listener_state) = match manager.listen_status_changes(wake_sender) {
        Ok(listener) => (Some(listener), ThermalListenerState::Registered),
        Err(status) => {
            tracing::warn!(target: "fasticamera::shell", status, "подписка на тепловой статус отвергнута, только опрос");
            (None, ThermalListenerState::RegistrationFailed { status })
        }
    };
    let worker_manager = Arc::clone(&manager);
    let spawned = thread::Builder::new()
        .name("fasticamera-thermal".to_owned())
        .spawn(move || run_monitor(&worker_manager, listener, &wake_receiver, poll_interval, &events));
    let listener_state = match spawned {
        Ok(_detached) => listener_state,
        Err(error) => {
            // Замыкание с подпиской уничтожено вместе с ошибкой — подписки больше нет, и
            // показания не идут; состояние говорит об этом честно, а не «registered».
            tracing::error!(target: "fasticamera::shell", %error, "поток теплового монитора не создан");
            ThermalListenerState::MonitorThreadFailed {
                reason: error.to_string(),
            }
        }
    };
    ThermalMonitor {
        manager: Some(manager),
        listener_state,
    }
}

fn run_monitor(
    manager: &ThermalManager,
    // Подписка живёт, пока работает поток (то есть весь процесс).
    _listener: Option<StatusListener>,
    wake: &Receiver<()>,
    poll_interval: Duration,
    events: &ShellEventSender,
) {
    loop {
        match read_reading(manager) {
            Some(reading) => {
                if events.send(ShellEvent::ThermalStatus(reading)) == SendOutcome::ReceiverGone {
                    tracing::info!(target: "fasticamera::shell", "получатель событий ушёл, тепловой монитор остановлен");
                    return;
                }
            }
            None => tracing::debug!(target: "fasticamera::shell", "тепловое показание пропущено"),
        }
        match wake.recv_timeout(poll_interval) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            // Подписки нет (отправитель уничтожен) — только опрос по таймеру.
            Err(RecvTimeoutError::Disconnected) => thread::sleep(poll_interval),
        }
    }
}

fn read_reading(manager: &ThermalManager) -> Option<ThermalReading> {
    match manager.current_status() {
        Ok(status) => Some(ThermalReading {
            status,
            headroom: manager.headroom(HeadroomForecast::NOW),
        }),
        Err(error) => {
            tracing::warn!(target: "fasticamera::shell", %error, "тепловой статус не прочитан");
            None
        }
    }
}
