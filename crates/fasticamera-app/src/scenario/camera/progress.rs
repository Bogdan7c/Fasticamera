//! Общие кирпичики сценариев камеры: дедлайны и журнал событий движка с временем от старта.
//!
//! Сценарии камеры не блокируют маршрутизатор: они двигаются по тикам, опрашивая снимок и события
//! движка. Здесь то, что у всех трёх одинаково: «срок вышел?» и «что движок рассказал и когда».

use std::time::{Duration, Instant};

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::stream::FrameRateMechanism;
use camera_core::CameraEvent;
use serde::Serialize;

/// Срок ожидания: точка во времени, после которой ожидание считается проваленным.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    at: Instant,
}

impl Deadline {
    /// Срок через `millis` мс после `now`.
    pub fn after_millis(now: Instant, millis: u64) -> Self {
        Self {
            at: now + Duration::from_millis(millis),
        }
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.at
    }
}

/// Имя рычага частоты для отчётов и проверок (совпадает с serde-тегом `FrameRateMechanism`).
pub fn lever_name(mechanism: &FrameRateMechanism) -> &'static str {
    match mechanism {
        FrameRateMechanism::HalDefault => "hal_default",
        FrameRateMechanism::PublicAeTargetRange { .. } => "public_ae_target_range",
        FrameRateMechanism::VendorSensorFrameDurationOverride { .. } => "vendor_sensor_frame_duration_override",
    }
}

/// Событие движка в сжатом виде для отчёта (полные состояния с размерами буферов в отчёт не нужны).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EventNote {
    /// Итог команды: имя и исход (JSON варианта `CommandOutcome`).
    Command {
        name: &'static str,
        outcome: serde_json::Value,
    },
    State {
        state: &'static str,
    },
    Unavailable {
        reason: CameraUnavailableReason,
    },
    Recovered {
        reason: CameraUnavailableReason,
        attempts: u32,
    },
    StreamConfigured {
        camera: String,
        lever: &'static str,
    },
    StreamConfigurationFailed {
        failure: String,
    },
}

impl EventNote {
    /// Сжатая запись события движка.
    pub fn of(event: &CameraEvent) -> Self {
        match event {
            CameraEvent::CommandHandled { name, outcome, .. } => EventNote::Command {
                name,
                outcome: serde_json::to_value(outcome).unwrap_or(serde_json::Value::Null),
            },
            CameraEvent::StateChanged { state } => EventNote::State { state: state.name() },
            CameraEvent::Unavailable { reason } => EventNote::Unavailable { reason: *reason },
            CameraEvent::Recovered { reason, attempts } => EventNote::Recovered {
                reason: *reason,
                attempts: *attempts,
            },
            CameraEvent::StreamConfigured { stream } => EventNote::StreamConfigured {
                camera: stream.stream.camera.to_string(),
                lever: lever_name(&stream.frame_rate_mechanism),
            },
            CameraEvent::StreamConfigurationFailed { failure, .. } => EventNote::StreamConfigurationFailed {
                failure: failure.to_string(),
            },
        }
    }
}

/// Запись журнала: что случилось и через сколько мс от старта журнала.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimedNote {
    pub at_ms: u64,
    #[serde(flatten)]
    pub note: EventNote,
}

/// Журнал событий движка, идущий от момента старта сценария (или шага).
#[derive(Debug, Clone)]
pub struct EventLog {
    started: Instant,
    notes: Vec<TimedNote>,
}

impl EventLog {
    pub fn started_at(started: Instant) -> Self {
        Self {
            started,
            notes: Vec::new(),
        }
    }

    /// Миллисекунды от старта журнала.
    pub fn elapsed_ms(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.started).as_millis()).unwrap_or(u64::MAX)
    }

    /// Записывает события, пришедшие к моменту `now`.
    pub fn absorb(&mut self, now: Instant, events: &[CameraEvent]) {
        let at_ms = self.elapsed_ms(now);
        self.notes.extend(events.iter().map(|event| TimedNote {
            at_ms,
            note: EventNote::of(event),
        }));
    }

    pub fn notes(&self) -> &[TimedNote] {
        &self.notes
    }
}

#[cfg(test)]
#[path = "progress_tests.rs"]
mod tests;
