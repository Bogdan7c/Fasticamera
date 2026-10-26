//! Состояние камеры глазами внешнего мира (UI, приложение, smoke).

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ConfiguredStream, ResolvedStream};
use serde::Serialize;

use crate::backend::BackendFailure;

/// Почему камера закрыта.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedReason {
    /// Никто не просил камеру (или попросили закрыть).
    NotRequested,
    /// Приложение не на переднем плане: камера отпущена, желаемое состояние сохранено
    /// и восстановится при возврате (Android не даёт камеру фоновым приложениям).
    AppInBackground,
}

/// Как движок собирается выйти из восстановимой ошибки (решение 39).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RecoveryPlan {
    /// Ждём, когда система скажет «камера свободна» (вытеснение, камера занята).
    WaitForCameraAvailable,
    /// Повтор по таймеру: попытка номер `attempt` через `delay_ms` от входа в ошибку.
    RetryAfterDelay { attempt: u32, delay_ms: u64 },
    /// Нужен внешний шаг (разрешение, политика): повторим при возврате на передний план.
    WaitForExternalAction,
}

/// Почему движок сдался.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FatalCause {
    /// Попытки восстановления кончились (лимит из конфига).
    RecoveryExhausted {
        reason: CameraUnavailableReason,
        attempts: u32,
    },
    /// Каталог камер прочитать не удалось.
    CatalogUnavailable { failure: BackendFailure },
    /// Бэкенд не создался на потоке камеры.
    BackendUnavailable { failure: BackendFailure },
    /// Итерация движка запаниковала; движок дальше не работает.
    EnginePanicked { message: String },
}

/// Ошибка камеры: восстановимая (движок сам выйдет) или окончательная.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraError {
    Recoverable {
        reason: CameraUnavailableReason,
        recovery: RecoveryPlan,
    },
    Fatal {
        cause: FatalCause,
    },
}

/// Состояние камеры.
///
/// Переходы: `Closed → Opening → Opened → Configuring → Streaming → Reconfiguring → …`,
/// из любого рабочего — в `Error`, из `Error(Recoverable)` — обратно в `Opening`.
/// `Opened` — устройство открыто, но режим потока ещё не задан.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraState {
    Closed {
        reason: ClosedReason,
    },
    Opening {
        camera: CameraId,
    },
    Opened {
        camera: CameraId,
    },
    Configuring {
        stream: ResolvedStream,
    },
    Streaming {
        stream: ConfiguredStream,
    },
    /// Поток шёл и перестраивается (смена режима или камеры): картинка вернётся сама.
    Reconfiguring {
        target: ResolvedStream,
    },
    Error {
        error: CameraError,
    },
}

impl CameraState {
    /// Имя варианта для логов и стадий smoke (совпадает с serde-тегом).
    pub fn name(&self) -> &'static str {
        match self {
            CameraState::Closed { .. } => "closed",
            CameraState::Opening { .. } => "opening",
            CameraState::Opened { .. } => "opened",
            CameraState::Configuring { .. } => "configuring",
            CameraState::Streaming { .. } => "streaming",
            CameraState::Reconfiguring { .. } => "reconfiguring",
            CameraState::Error { .. } => "error",
        }
    }
}
