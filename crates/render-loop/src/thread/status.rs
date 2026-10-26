//! Снимок состояния потока рендера для сценариев и smoke-проверок.

use std::collections::BTreeMap;
use std::time::Duration;

use shell_contract::commands::{CommandOutcome, FrameRateOutcome, ScreenWake};
use shell_contract::surface::SurfaceGeneration;

use crate::backend::DisplayInfo;
use crate::pacing::PacingSnapshot;

/// Общее состояние потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderState {
    /// Surface нет, поток спит.
    WaitingForSurface,
    /// Surface есть, размера ещё нет.
    AwaitingSize,
    /// Кадры показываются.
    Presenting,
    /// Surface присоединена, но показ на паузе.
    Paused,
    /// Surface есть у Android, но бэкенд её не держит (отказ attach / потеря surface).
    SurfaceUnusable,
    /// Поток неисправен: кадров нет, запросы освобождения подтверждаются сразу.
    Faulted,
    /// Поток завершён.
    Stopped,
}

/// Причина неисправности.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Паника внутри вызова бэкенда.
    BackendPanic,
    /// Источник тиков остановился навсегда.
    ClockStopped,
    /// Паника в самом цикле потока вне вызова бэкенда (часы кадров, пейсинг, машина состояний,
    /// команды оболочки). Поток переведён в неисправное состояние, а не умер молча.
    WorkerPanic,
}

/// Описание неисправности.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultInfo {
    pub kind: FaultKind,
    pub message: String,
}

/// Последние известные показатели здоровья бэкенда.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendHealth {
    pub live_objects: u64,
    /// Живые объекты по видам (виды с нулём не перечисляются); см. `BackendDiagnostics`.
    pub live_objects_by_kind: BTreeMap<String, u64>,
    pub validation_errors: u64,
    pub validation_warnings: u64,
    /// Сколько раз `detach` бэкенда завершился ошибкой.
    pub detach_failures: u64,
    /// Срабатывания известных отходов validation по VUID (справочно, не ошибки).
    pub known_deviations: BTreeMap<String, u64>,
}

/// Учёт управления «не гасить экран» (поток шлёт команду только при смене желаемого режима).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenWakeStatus {
    /// Режим, который поток последним запросил у платформы (изначально `AllowSleep`:
    /// ничего не просили).
    pub requested: ScreenWake,
    /// Итог последней отправленной команды (`None` — команд не было).
    pub last_outcome: Option<CommandOutcome>,
    /// Сколько команд отправлено за всё время; растёт только на переходах, не на кадрах.
    pub commands_sent: u64,
}

/// Снимок состояния (копируется под коротким замком).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderStatus {
    pub state: RenderState,
    /// Поколение текущей surface, если она есть.
    pub generation: Option<SurfaceGeneration>,
    /// Сколько кадров поставлено на показ.
    pub frames_presented: u64,
    /// Сколько раз кадр пропущен из-за таймаута acquire.
    pub acquire_timeouts: u64,
    /// Сколько раз пересоздавалась цепочка показа (смена размера, NeedsRecreate).
    pub recreations: u64,
    /// Сколько surface успешно присоединено за всё время.
    pub surfaces_attached: u64,
    /// Сколько квитанций освобождения подтверждено.
    pub releases_confirmed: u64,
    /// Задержка последнего освобождения: от попадания запроса в ящик до подтверждения.
    pub last_release_latency: Option<Duration>,
    /// Сколько `changed` проигнорировано как устаревшие.
    pub stale_changes: u64,
    /// Сколько ошибок бэкенда (attach/resize/кадр) посчитано.
    pub backend_errors: u64,
    /// Текст последней ошибки бэкенда.
    pub last_backend_error: Option<String>,
    pub pacing: PacingSnapshot,
    /// Период обновления, по которому сейчас судит пейсинг: реальный от бэкенда или запасной
    /// из `preferred_frame_rate_hz`, пока реальный неизвестен.
    pub pacing_refresh_period: Duration,
    /// Параметры текущей цепочки показа по данным бэкенда (`None` — её нет).
    pub display: Option<DisplayInfo>,
    pub backend: BackendHealth,
    /// Что ответила платформа на просьбу о частоте кадров (однократно при старте).
    pub frame_rate_request: Option<FrameRateOutcome>,
    /// Управление блокировкой гашения экрана на время показа.
    pub screen_wake: ScreenWakeStatus,
    pub fault: Option<FaultInfo>,
}

impl RenderStatus {
    pub(super) fn initial(pacing_refresh_period: Duration) -> Self {
        Self {
            state: RenderState::WaitingForSurface,
            generation: None,
            frames_presented: 0,
            acquire_timeouts: 0,
            recreations: 0,
            surfaces_attached: 0,
            releases_confirmed: 0,
            last_release_latency: None,
            stale_changes: 0,
            backend_errors: 0,
            last_backend_error: None,
            pacing: PacingSnapshot::empty(),
            pacing_refresh_period,
            display: None,
            backend: BackendHealth::default(),
            frame_rate_request: None,
            screen_wake: ScreenWakeStatus {
                requested: ScreenWake::AllowSleep,
                last_outcome: None,
                commands_sent: 0,
            },
            fault: None,
        }
    }
}
