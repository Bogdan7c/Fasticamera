//! Команды движку (intent-методы [`crate::CameraHandle`]) и их явные итоги.

use camera_contract::catalog::TargetUnavailable;
use camera_contract::control::{CaptureIntent, ManualControl};
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::stream::StreamMode;
use serde::Serialize;

use crate::backend::UnsupportedStream;

/// Номер команды: выдаётся при отправке, по нему итог находится в событиях.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CommandId(pub u64);

/// Команда движку. Каждая меняет **желаемое** состояние; движок сам доводит до него
/// реальное, в том числе после вытеснения (решение 39).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraCommand {
    /// Хотим эту камеру открытой.
    Open {
        target: CameraTarget,
    },
    /// Хотим этот режим потока на желаемой камере.
    Configure {
        mode: StreamMode,
    },
    /// Хотим, чтобы шёл повторяющийся запрос.
    StartStreaming {
        intent: CaptureIntent,
    },
    StopStreaming,
    /// Перейти на камеру другой роли, сохранив режим и поток.
    SwitchTo {
        role: LensRole,
    },
    ApplyControls {
        controls: ManualControl,
    },
    /// Камера не нужна: закрыть и забыть желаемое.
    Close,
    /// Приложение ушло с переднего плана: отпустить камеру, желаемое сохранить.
    EnterBackground,
    /// Приложение снова на переднем плане: восстановить желаемое, счётчик попыток с нуля.
    EnterForeground,
}

impl CameraCommand {
    pub fn name(&self) -> &'static str {
        match self {
            CameraCommand::Open { .. } => "open",
            CameraCommand::Configure { .. } => "configure",
            CameraCommand::StartStreaming { .. } => "start_streaming",
            CameraCommand::StopStreaming => "stop_streaming",
            CameraCommand::SwitchTo { .. } => "switch_to",
            CameraCommand::ApplyControls { .. } => "apply_controls",
            CameraCommand::Close => "close",
            CameraCommand::EnterBackground => "enter_background",
            CameraCommand::EnterForeground => "enter_foreground",
        }
    }
}

/// Почему команда противоречит текущему желаемому состоянию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidStateReason {
    /// Нужна желаемая камера (`open`), а её нет.
    NoCameraRequested,
    /// Нужен режим потока (`configure`), а его нет.
    NoStreamModeRequested,
    /// Движок в окончательной ошибке и команд не исполняет.
    EngineFailed,
}

/// Почему команду нельзя исполнить на этом телефоне.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UnsupportedCommand {
    Target(TargetUnavailable),
    Stream(UnsupportedStream),
}

/// Итог команды. Не `bool`: «уже так», «невозможно» и «не в том состоянии» —
/// разные ответы для вызывающего (правило 6 `AGENTS.md`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CommandOutcome {
    /// Желаемое состояние изменено; движок доводит до него реальное.
    Accepted,
    /// Желаемое уже такое (например, повторный `open` той же камеры) — ничего не менялось.
    AlreadyInState,
    /// На этом телефоне невозможно (нет роли, формат не предлагается…). Желаемое не менялось.
    Unsupported(UnsupportedCommand),
    /// Противоречит текущему желаемому состоянию. Желаемое не менялось.
    InvalidState(InvalidStateReason),
}

/// Команду не удалось даже отправить. Тоже не `bool`: очередь полна (подождать)
/// и движок остановлен (бесполезно ждать) — разные ситуации.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommandRejected {
    #[error("очередь команд камеры полна")]
    QueueFull,
    #[error("движок камеры остановлен")]
    EngineStopped,
}
