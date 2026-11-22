//! Снимок состояния потока рендера для сценариев и smoke-проверок.

use crate::still::StillCommandLog;
use std::collections::BTreeMap;
use std::time::Duration;

use shell_contract::commands::{CommandOutcome, FrameRateOutcome, ScreenWake};
use shell_contract::surface::SurfaceGeneration;

use super::adpf::{AdpfCommandLog, AdpfDiagnostics};
use super::present_target_control::PresentTargetCommandLog;
use super::present_target_stats::PresentTargetDiagnostics;
use crate::backend::{BackendDiagnostics, DisplayInfo, PastPresentation};
use crate::camera_diagnostics::CameraStreamDiagnostics;
use crate::pacing::PacingSnapshot;
use crate::viewfinder::{ViewfinderCommandLog, ViewfinderDiagnostics};

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
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Показ кадров камеры; `None` — показывается тестовый паттерн.
    pub camera: Option<CameraStreamDiagnostics>,
    /// Видоискатель (рисовальщик, стадии ISP, время GPU, счётчики); `None` — камеры нет.
    pub viewfinder: Option<ViewfinderDiagnostics>,
    /// Снимки (сессия 14): выборы кадра, захваты, проявления, итоги, потери GPU; `None` — снимков нет.
    pub still: Option<crate::still_diagnostics::StillDiagnostics>,
}

impl BackendHealth {
    /// Показатели здоровья из диагностики бэкенда.
    pub(super) fn from_diagnostics(diagnostics: &BackendDiagnostics) -> Self {
        Self {
            live_objects: diagnostics.live_objects,
            live_objects_by_kind: diagnostics.live_objects_by_kind.clone(),
            validation_errors: diagnostics.validation_errors,
            validation_warnings: diagnostics.validation_warnings,
            detach_failures: diagnostics.detach_failures,
            known_deviations: diagnostics.known_deviations.clone(),
            camera: diagnostics.camera.clone(),
            viewfinder: diagnostics.viewfinder.clone(),
            still: diagnostics.still.clone(),
        }
    }
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
    /// Последние реальные времена показа (старые первыми, не больше
    /// `RenderLoopConfig::recent_presentations_capacity`). Часы `MONOTONIC`: сопоставлять с меткой
    /// сенсора (`BOOTTIME`) можно только через измеренное смещение часов — это делает приложение.
    pub recent_presentations: Vec<PastPresentation>,
    /// Выбор линии кадра, опоздание показов относительно задуманного vsync, время в acquire и
    /// стоимость кадра (сессия 07). Обновляется при опросе диагностики.
    pub present_target: PresentTargetDiagnostics,
    /// Команды выбору линии кадра и позднему захвату (A/B задержки в одном прогоне) и их итоги.
    pub present_target_commands: PresentTargetCommandLog,
    /// Команды видоискателю и их итоги (сессия 07).
    pub viewfinder_commands: ViewfinderCommandLog,
    /// Команды снимка (сессия 14): принятые, неподдержанные, отпущенные цели.
    pub still_commands: StillCommandLog,
    /// Подсказки производительности системе (ADPF): режим, сессия, отчёты, отказы.
    pub adpf: AdpfDiagnostics,
    /// Учёт команд ADPF.
    pub adpf_commands: AdpfCommandLog,
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
            recent_presentations: Vec::new(),
            present_target: PresentTargetDiagnostics::default(),
            present_target_commands: PresentTargetCommandLog::default(),
            viewfinder_commands: ViewfinderCommandLog::default(),
            still_commands: StillCommandLog::default(),
            adpf: AdpfDiagnostics::default(),
            adpf_commands: AdpfCommandLog::default(),
        }
    }

    /// Запоминает реальное время показа, вытесняя самые старые записи сверх `capacity`.
    pub(super) fn note_presentation(&mut self, presentation: PastPresentation, capacity: usize) {
        self.recent_presentations.push(presentation);
        if self.recent_presentations.len() > capacity {
            let excess = self.recent_presentations.len() - capacity;
            self.recent_presentations.drain(..excess);
        }
    }
}
