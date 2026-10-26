//! Наблюдение за движком камеры из сценариев: чем кончилась попытка и что считать отказом.
//!
//! Все три сценария камеры одинаково читают события движка (отказ команды, отказ конфигурации,
//! окончательная ошибка) и одинаково отправляют команды. Здесь общий словарь отказов
//! [`RunFailure`] и функции, превращающие события в отказы; вердикты остаются в отчётах сценариев.

use camera_core::command::CommandOutcome;
use camera_core::{CameraError, CameraEvent, CameraState, CameraStatus, CommandRejected};
use serde::Serialize;

/// Почему шаг (или весь прогон) не дошёл до цели.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RunFailure {
    /// Команду не приняла очередь движка.
    CommandRejected { command: &'static str, rejected: String },
    /// Движок принял команду, но отказал по существу (нет роли, режим не поддерживается, ...).
    CommandNotAccepted {
        command: &'static str,
        outcome: serde_json::Value,
    },
    /// Режим не сконфигурировался на этой камере.
    ConfigurationFailed { failure: String },
    /// Движок в окончательной ошибке.
    EngineFatal { cause: String },
    /// `Streaming` не наступил за срок.
    NeverStreaming { last_state: &'static str },
    /// Предыдущая камера не закрылась за срок, шаг не начинался.
    PreviousCameraDidNotClose { last_state: &'static str },
    /// `camera-evict`: вытеснения не случилось за срок.
    #[cfg(any(test, feature = "debug-scenarios"))]
    NeverEvicted { last_state: &'static str },
    /// `camera-evict`: после вытеснения поток не вернулся за срок.
    #[cfg(any(test, feature = "debug-scenarios"))]
    NeverRecovered { last_state: &'static str },
}

impl RunFailure {
    /// Описание для списка нарушений.
    pub fn describe(&self) -> String {
        match self {
            RunFailure::CommandRejected { command, rejected } => {
                format!("команда {command} не принята движком: {rejected}")
            }
            RunFailure::CommandNotAccepted { command, outcome } => {
                format!("движок отказал в команде {command}: {outcome}")
            }
            RunFailure::ConfigurationFailed { failure } => format!("режим не сконфигурировался: {failure}"),
            RunFailure::EngineFatal { cause } => format!("движок в окончательной ошибке: {cause}"),
            RunFailure::NeverStreaming { last_state } => {
                format!("не дошёл до Streaming за отведённое время (состояние: {last_state})")
            }
            RunFailure::PreviousCameraDidNotClose { last_state } => {
                format!("предыдущая камера не закрылась (состояние: {last_state}), шаг не выполнялся")
            }
            #[cfg(any(test, feature = "debug-scenarios"))]
            RunFailure::NeverEvicted { last_state } => {
                format!("вытеснения не было за отведённое время (состояние: {last_state})")
            }
            #[cfg(any(test, feature = "debug-scenarios"))]
            RunFailure::NeverRecovered { last_state } => {
                format!("поток не восстановился за отведённое время (состояние: {last_state})")
            }
        }
    }
}

/// Очередь движка не приняла команду.
pub fn rejected_failure(command: &'static str, rejected: CommandRejected) -> RunFailure {
    RunFailure::CommandRejected {
        command,
        rejected: rejected.to_string(),
    }
}

/// Событие, делающее прогон проваленным (отказ движка по существу), или `None`.
pub fn failure_of_event(event: &CameraEvent) -> Option<RunFailure> {
    match event {
        CameraEvent::CommandHandled { name, outcome, .. } => match outcome {
            CommandOutcome::Accepted | CommandOutcome::AlreadyInState => None,
            CommandOutcome::Unsupported(_) | CommandOutcome::InvalidState(_) => Some(RunFailure::CommandNotAccepted {
                command: name,
                outcome: serde_json::to_value(outcome).unwrap_or(serde_json::Value::Null),
            }),
        },
        CameraEvent::StreamConfigurationFailed { failure, .. } => Some(RunFailure::ConfigurationFailed {
            failure: failure.to_string(),
        }),
        CameraEvent::StateChanged { .. }
        | CameraEvent::Unavailable { .. }
        | CameraEvent::Recovered { .. }
        | CameraEvent::StreamConfigured { .. } => None,
    }
}

/// Движок в окончательной ошибке — дальше ждать бессмысленно.
pub fn fatal_failure(status: &CameraStatus) -> Option<RunFailure> {
    match &status.state {
        CameraState::Error {
            error: CameraError::Fatal { cause },
        } => Some(RunFailure::EngineFatal {
            cause: format!("{cause:?}"),
        }),
        _ => None,
    }
}

/// Причины недоступности из пачки событий.
pub fn unavailable_reasons(events: &[CameraEvent]) -> Vec<camera_contract::availability::CameraUnavailableReason> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::Unavailable { reason } => Some(*reason),
            _ => None,
        })
        .collect()
}
