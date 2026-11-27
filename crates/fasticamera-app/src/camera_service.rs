//! Сервис камеры для маршрутизатора и сценариев: ручка движка, диагностика бэкенда, политика фона.
//!
//! Composition root держит здесь всё, что приложению нужно знать о камере, но сам решений о камере
//! не принимает: желаемое состояние ведёт движок (`camera-core`), платформа — бэкенд
//! (`camera-android`). Сервис даёт два узких входа.
//! * Для **маршрутизатора** — события жизненного цикла приложения: [`CameraServices::app_resumed`],
//!   [`CameraServices::app_paused`] (с учётом [`CameraLifecyclePolicy`]) и [`CameraServices::scenario_ended`]
//!   (возврат политики по умолчанию и закрытие камеры, которую открывал сценарий; если включён
//!   автозапуск видоискателя — возврат его желаемого состояния, см. `viewfinder_autostart`).
//! * Для **composition root** — [`CameraServices::start_product_viewfinder`]: временная (до сессии 12)
//!   политика «приложение стартует с живым видоискателем».
//! * Для **сценариев** — доступ к ручке движка и снимку диагностики ([`CameraServices::running`]),
//!   запрос удержания камеры в фоне.
//!
//! Ничего здесь не блокируется: команды уходят в ограниченную очередь движка, а отказ очереди
//! (`QueueFull`, `EngineStopped`) — это значение [`CameraDispatch`] и счётчик, не паника и не ожидание.
//!
//! Если движок не поднялся (нет разрешения на поток, неверный конфиг), приложение живёт дальше:
//! сервис хранит причину ([`CameraEngineAccess::Unavailable`]), а сценарии честно сообщают её в отчёте.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use camera_android::{BackendDiagnostics, CpuProbeMode, DiagnosticsSnapshot};
use camera_contract::white_reading::WhiteReadingBoard;
use camera_contract::zoom::LensIntentChannel;
use camera_core::{CameraHandle, CameraRuntime, CommandId, CommandRejected};
use look_contract::LookWhiteStopsBoard;
use raw_frame_stream::ViewfinderPacingSwitch;
use scope_contract::ScopeLabelsBoard;
use serde::Serialize;

use crate::auto_exposure_link::{EngineBinding, ExposureControllerChoice};
use crate::camera_lifecycle::{CameraLifecyclePolicy, CameraLifecycleState, PolicyRestore};
use crate::camera_scenario_config::CameraScenarioConfig;
use crate::raw_stream_services::RawStreamServices;
use crate::viewfinder_autostart::{UserLens, ViewfinderAutostart, ViewfinderAutostartOutcome};
use crate::white_memory::{WhiteMemoryDiagnostics, WhiteMemoryStats};

const LOG_TARGET: &str = "fasticamera::camera::service";

/// Источник снимков диагностики бэкенда. Узкий контракт: сценарии не знают, что за ним стоит
/// (на телефоне — `Arc<BackendDiagnostics>` Camera2, в тестах — тот же тип, заполняемый вручную).
pub trait BackendDiagnosticsSource: Send + Sync {
    fn snapshot(&self) -> DiagnosticsSnapshot;
}

impl BackendDiagnosticsSource for BackendDiagnostics {
    fn snapshot(&self) -> DiagnosticsSnapshot {
        BackendDiagnostics::snapshot(self)
    }
}

/// Запущенный движок камеры. Владеет потоком `fasticamera-camera` через [`CameraRuntime`]: пока жив
/// сервис, жив поток; уничтожение сервиса останавливает поток (с таймаутом из конфига движка).
pub struct RunningCamera {
    handle: CameraHandle,
    diagnostics: Arc<dyn BackendDiagnosticsSource>,
    /// Привязка толчка рендера к движку (наш AE). Объявлена раньше `_runtime`: при падении сначала
    /// отвязывается получатель, потом останавливается поток движка.
    _exposure_binding: Option<EngineBinding>,
    /// Привязка толчка ответов источников белого: так же отвязывается до остановки потока движка.
    _white_binding: Option<EngineBinding>,
    /// Только держим поток живым. `Mutex` нужен лишь ради `Sync`: `CameraRuntime` отправляется между
    /// потоками, но не разделяется (внутри приёмник канала), а сервис разделяется маршрутизатором и сценариями.
    _runtime: Mutex<CameraRuntime>,
}

impl RunningCamera {
    pub fn new(runtime: CameraRuntime, handle: CameraHandle, diagnostics: Arc<dyn BackendDiagnosticsSource>) -> Self {
        Self {
            handle,
            diagnostics,
            _exposure_binding: None,
            _white_binding: None,
            _runtime: Mutex::new(runtime),
        }
    }

    /// Запоминает привязку толчка рендера: она живёт, пока жив движок.
    pub fn with_exposure_binding(mut self, binding: EngineBinding) -> Self {
        self._exposure_binding = Some(binding);
        self
    }

    /// Запоминает привязку толчка ответов источников белого.
    pub fn with_white_binding(mut self, binding: EngineBinding) -> Self {
        self._white_binding = Some(binding);
        self
    }

    /// Ручка движка (команды, снимок состояния, события).
    pub fn handle(&self) -> &CameraHandle {
        &self.handle
    }

    /// Свежий снимок диагностики бэкенда.
    pub fn diagnostics_snapshot(&self) -> DiagnosticsSnapshot {
        self.diagnostics.snapshot()
    }
}

/// Есть ли у приложения движок камеры.
#[derive(Clone)]
pub enum CameraEngineAccess {
    Running(Arc<RunningCamera>),
    /// Движок не поднялся; приложение живёт без камеры, причина видна сценариям и логу.
    Unavailable {
        reason: String,
    },
}

/// Движка камеры нет: причина, с которой он не поднялся.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("камера недоступна: {reason}")]
pub struct CameraUnavailable {
    pub reason: String,
}

/// Куда делась команда жизненного цикла. Не `bool`: «отправлено», «политика оставила камеру»,
/// «движка нет» и отказ очереди — разные ответы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraDispatch {
    /// Команда принята очередью движка под этим номером.
    Sent(CommandId),
    /// Политика `HoldInBackgroundForDiagnostics`: команду «уйти в фон» не отправляли.
    KeptOpenByPolicy,
    /// Движка камеры нет (не поднялся): слать некому.
    EngineUnavailable,
    /// Очередь отказала.
    Rejected(CommandRejected),
}

/// Счётчики отправки команд жизненного цикла (накопительные за процесс).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CameraDispatchCounters {
    pub sent: u64,
    pub kept_open_by_policy: u64,
    pub engine_unavailable: u64,
    pub rejected_queue_full: u64,
    pub rejected_engine_stopped: u64,
}

#[derive(Debug, Default)]
struct DispatchCounterCells {
    sent: AtomicU64,
    kept_open_by_policy: AtomicU64,
    engine_unavailable: AtomicU64,
    rejected_queue_full: AtomicU64,
    rejected_engine_stopped: AtomicU64,
}

impl DispatchCounterCells {
    fn record(&self, dispatch: CameraDispatch) {
        let cell = match dispatch {
            CameraDispatch::Sent(_) => &self.sent,
            CameraDispatch::KeptOpenByPolicy => &self.kept_open_by_policy,
            CameraDispatch::EngineUnavailable => &self.engine_unavailable,
            CameraDispatch::Rejected(CommandRejected::QueueFull) => &self.rejected_queue_full,
            CameraDispatch::Rejected(CommandRejected::EngineStopped) => &self.rejected_engine_stopped,
        };
        cell.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> CameraDispatchCounters {
        CameraDispatchCounters {
            sent: self.sent.load(Ordering::Relaxed),
            kept_open_by_policy: self.kept_open_by_policy.load(Ordering::Relaxed),
            engine_unavailable: self.engine_unavailable.load(Ordering::Relaxed),
            rejected_queue_full: self.rejected_queue_full.load(Ordering::Relaxed),
            rejected_engine_stopped: self.rejected_engine_stopped.load(Ordering::Relaxed),
        }
    }
}

/// Приложение на переднем плане или нет — вход для решения «отпускать ли камеру».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppVisibility {
    Foreground,
    Background,
}

/// Что сделал сервис, когда сценарий закончился.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScenarioEndEffects {
    pub policy: PolicyRestore,
    /// `Some` — сценарий заявлял права на камеру, и она закрывалась этой командой.
    pub close_claimed_camera: Option<CameraDispatch>,
    /// `Some` — политика вернулась из удержания, а приложение в фоне: камера отпускалась этой командой.
    pub release_after_hold: Option<CameraDispatch>,
    /// Отладочная проба CPU-доступа была взведена сценарием и сейчас выключена. Проба не должна
    /// пережить сценарий ни при каком его конце: иначе она копировала бы кадры в рабочем режиме.
    pub cpu_probe_disarmed: bool,
    /// `Some` — камера сценария закрыта (`close` принят очередью): так кончился возврат
    /// видоискателя продукта (`Disabled`, если автозапуск выключен — камера остаётся закрытой).
    pub viewfinder_restored: Option<ViewfinderAutostartOutcome>,
    /// `Some` — сценарий менял политику подачи кадров видоискателя, и она возвращена (итог переключения).
    pub viewfinder_pacing_restored: Option<ViewfinderPacingSwitch>,
}

/// Всё о камере для маршрутизатора и сценариев. Дёшево клонируется.
#[derive(Clone)]
pub struct CameraServices {
    engine: CameraEngineAccess,
    lifecycle: Arc<CameraLifecycleState>,
    counters: Arc<DispatchCounterCells>,
    scenarios: Arc<CameraScenarioConfig>,
    raw_stream: RawStreamServices,
    viewfinder_autostart: Arc<ViewfinderAutostart>,
    exposure_controller: ExposureControllerChoice,
    /// Ящик намерений объектива (сессия 13): его публикует экран камеры и автозапуск, читает движок.
    lens_intents: LensIntentChannel,
    /// Объектив пользователя на момент заявки сценария: по концу сценария видоискатель открывается им.
    user_lens_before_scenario: Arc<Mutex<UserLens>>,
    /// Счётчики и последняя ошибка записи памяти белой (пусты, если память не открывалась).
    white_memory: WhiteMemoryDiagnostics,
    /// Доска показаний источников белого: рендер пишет, экран камеры читает («≈ 3100» на кнопке).
    white_readings: WhiteReadingBoard,
    /// Доска упоров ручек теплоты и оттенка: рендер пишет, приложение читает для экрана калибровки.
    look_white_stops: LookWhiteStopsBoard,
    /// Доска подписей прибора (решения 326, 327): рендер пишет, приложение читает для экрана калибровки.
    scope_labels: ScopeLabelsBoard,
}

impl CameraServices {
    /// Сервис с отсоединённым потоком кадров (см. [`CameraServices::with_raw_stream`]).
    pub fn new(engine: CameraEngineAccess, scenarios: CameraScenarioConfig) -> Self {
        Self {
            engine,
            lifecycle: Arc::default(),
            counters: Arc::default(),
            scenarios: Arc::new(scenarios),
            raw_stream: RawStreamServices::detached(),
            viewfinder_autostart: Arc::new(ViewfinderAutostart::disabled()),
            exposure_controller: ExposureControllerChoice::default(),
            lens_intents: LensIntentChannel::new(),
            user_lens_before_scenario: Arc::new(Mutex::new(UserLens::NoneBeforeScenario)),
            white_memory: WhiteMemoryDiagnostics::default(),
            white_readings: WhiteReadingBoard::new(),
            look_white_stops: LookWhiteStopsBoard::new(),
            scope_labels: ScopeLabelsBoard::new(),
        }
    }

    /// Подключает доску показаний источников белого (тот же клон, что получил рендер).
    pub fn with_white_readings(mut self, white_readings: WhiteReadingBoard) -> Self {
        self.white_readings = white_readings;
        self
    }

    /// Подключает доску упоров ручек теплоты и оттенка (тот же клон, что получил рендер).
    pub fn with_look_white_stops(mut self, look_white_stops: LookWhiteStopsBoard) -> Self {
        self.look_white_stops = look_white_stops;
        self
    }

    /// Подключает доску подписей прибора (тот же клон, что получил рендер).
    pub fn with_scope_labels(mut self, scope_labels: ScopeLabelsBoard) -> Self {
        self.scope_labels = scope_labels;
        self
    }

    /// Доска подписей прибора (читающая сторона — [`crate::scope_labels::ScopeLabelsReader`]).
    pub fn scope_labels(&self) -> &ScopeLabelsBoard {
        &self.scope_labels
    }

    /// Доска упоров ручек теплоты и оттенка (читающая сторона — [`crate::look_white_stops::LookWhiteStopsReader`]).
    pub fn look_white_stops(&self) -> &LookWhiteStopsBoard {
        &self.look_white_stops
    }

    /// Доска показаний источников белого для фактов экрана.
    pub fn white_readings(&self) -> &WhiteReadingBoard {
        &self.white_readings
    }

    /// Подключает счётчики записи памяти белой (те же, что у потока записи).
    pub fn with_white_memory(mut self, diagnostics: WhiteMemoryDiagnostics) -> Self {
        self.white_memory = diagnostics;
        self
    }

    /// Счётчики записи памяти белой и последняя ошибка записи: отказ диска виден здесь, а не только в журнале.
    pub fn white_memory_stats(&self) -> WhiteMemoryStats {
        self.white_memory.snapshot()
    }

    /// Подключает ящик намерений объектива, который читает движок (композиция передаёт тот же клон движку).
    pub fn with_lens_intents(mut self, lens_intents: LensIntentChannel) -> Self {
        self.lens_intents = lens_intents;
        self
    }

    /// Ящик намерений объектива: экран камеры публикует в него вместо очереди команд движка.
    pub fn lens_intents(&self) -> &LensIntentChannel {
        &self.lens_intents
    }

    /// Запоминает, какой исполнитель экспозиции выбрала композиция (и почему не тот, что в конфиге).
    pub fn with_exposure_controller(mut self, choice: ExposureControllerChoice) -> Self {
        self.exposure_controller = choice;
        self
    }

    /// Какой исполнитель экспозиции реально у камеры: сценарии сверяют с ним то, что видят в движке.
    pub fn exposure_controller(&self) -> &ExposureControllerChoice {
        &self.exposure_controller
    }

    /// Подключает политику автозапуска видоискателя (по умолчанию выключена).
    pub fn with_viewfinder_autostart(mut self, autostart: ViewfinderAutostart) -> Self {
        self.viewfinder_autostart = Arc::new(autostart);
        self
    }

    /// Временная политика до сессии 12: выставить движку желаемое состояние живого видоискателя.
    /// Один раз за процесс (повтор — `AlreadyStarted`); фон и передний план дальше ведёт движок.
    pub fn start_product_viewfinder(&self) -> ViewfinderAutostartOutcome {
        self.viewfinder_autostart.start_once(self.engine_handle())
    }

    /// Подключает наблюдательные сервисы потока кадров (счётчики, проба CPU, часы).
    pub fn with_raw_stream(mut self, raw_stream: RawStreamServices) -> Self {
        self.raw_stream = raw_stream;
        self
    }

    /// Наблюдательные сервисы потока RAW-кадров.
    pub fn raw_stream(&self) -> &RawStreamServices {
        &self.raw_stream
    }

    /// Сервис без движка: камера недоступна по причине `reason`.
    pub fn unavailable(reason: impl Into<String>, scenarios: CameraScenarioConfig) -> Self {
        let reason = reason.into();
        tracing::error!(target: LOG_TARGET, reason = %reason, "камера недоступна, приложение работает без неё");
        Self::new(CameraEngineAccess::Unavailable { reason }, scenarios)
    }

    /// Запущенный движок или причина, по которой его нет.
    pub fn running(&self) -> Result<&RunningCamera, CameraUnavailable> {
        match &self.engine {
            CameraEngineAccess::Running(running) => Ok(running),
            CameraEngineAccess::Unavailable { reason } => Err(CameraUnavailable { reason: reason.clone() }),
        }
    }

    /// Параметры сценариев камеры.
    pub fn scenarios(&self) -> &CameraScenarioConfig {
        &self.scenarios
    }

    pub fn lifecycle_policy(&self) -> CameraLifecyclePolicy {
        self.lifecycle.policy()
    }

    pub fn dispatch_counters(&self) -> CameraDispatchCounters {
        self.counters.snapshot()
    }

    /// Сценарий начал пользоваться камерой: по его концу маршрутизатор камеру закроет.
    pub fn claim_camera_for_scenario(&self) {
        self.remember_user_lens_before_claim();
        self.lifecycle.claim_for_scenario();
    }

    /// Запоминает объектив пользователя ПЕРВОЙ заявкой сценария (повторная заявка того же сценария ничего не
    /// перезаписывает: к ней камера уже могла быть переключена самим сценарием). Нет потока и смены — запоминать нечего.
    fn remember_user_lens_before_claim(&self) {
        if self.lifecycle.has_scenario_claim() {
            return;
        }
        let lens = match self.running() {
            Ok(running) => UserLens::from_lens_status(&running.handle().status().lens),
            Err(_) => UserLens::NoneBeforeScenario,
        };
        *self
            .user_lens_before_scenario
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = lens;
    }

    /// Только отладочный сценарий на время своей работы: приложение в фоне камеру не отпускает.
    /// Возвращает политику по умолчанию маршрутизатор ([`CameraServices::scenario_ended`]).
    pub fn hold_in_background_for_diagnostics(&self) {
        self.remember_user_lens_before_claim();
        self.lifecycle.hold_in_background_for_diagnostics();
    }

    /// Приложение вернулось на передний план: движок восстанавливает желаемое состояние.
    /// Отправляется при любой политике (политика касается только ухода в фон).
    pub fn app_resumed(&self) -> CameraDispatch {
        self.dispatch(CameraHandle::enter_foreground)
    }

    /// Приложение ушло в фон: камера отпускается, если политика не велит держать.
    pub fn app_paused(&self) -> CameraDispatch {
        match self.lifecycle.policy() {
            CameraLifecyclePolicy::ReleaseInBackground => self.dispatch(CameraHandle::enter_background),
            CameraLifecyclePolicy::HoldInBackgroundForDiagnostics => {
                self.counters.record(CameraDispatch::KeptOpenByPolicy);
                CameraDispatch::KeptOpenByPolicy
            }
        }
    }

    /// Сценарий закончился любым путём: вернуть политику по умолчанию, закрыть камеру, которую
    /// он открывал, и отпустить её, если политика держала камеру в фоне, а приложение в фоне сейчас.
    pub fn scenario_ended(&self, visibility: AppVisibility) -> ScenarioEndEffects {
        let policy = self.lifecycle.restore_default();
        let cpu_probe_disarmed = self.raw_stream.cpu_probe_mode() != CpuProbeMode::Disarmed;
        self.raw_stream.disarm_cpu_probe();
        let viewfinder_pacing_restored = self.raw_stream.restore_viewfinder_pacing_after_scenario();
        let close_claimed_camera = self
            .lifecycle
            .take_scenario_claim()
            .then(|| self.dispatch(CameraHandle::close));
        // Камера сценария закрыта (желаемое забыто вместе с его ручными настройками) — вернуть
        // желаемое состояние видоискателя продукта, если автозапуск включён. Если `close` не ушёл,
        // открывать поверх желаемого сценария нельзя: в логе будет отказ `close`.
        let user_lens = std::mem::replace(
            &mut *self
                .user_lens_before_scenario
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            UserLens::NoneBeforeScenario,
        );
        let viewfinder_restored = match close_claimed_camera {
            Some(CameraDispatch::Sent(_)) => Some(
                self.viewfinder_autostart
                    .restore_after_scenario(self.engine_handle(), user_lens),
            ),
            Some(_) | None => None,
        };
        let release_after_hold = (policy == PolicyRestore::RestoredFromHold && visibility == AppVisibility::Background)
            .then(|| self.app_paused());
        ScenarioEndEffects {
            policy,
            close_claimed_camera,
            release_after_hold,
            cpu_probe_disarmed,
            viewfinder_restored,
            viewfinder_pacing_restored,
        }
    }

    /// Ручка движка или причина, по которой его нет.
    fn engine_handle(&self) -> Result<&CameraHandle, CameraUnavailable> {
        self.running().map(RunningCamera::handle)
    }

    fn dispatch(&self, send: impl FnOnce(&CameraHandle) -> Result<CommandId, CommandRejected>) -> CameraDispatch {
        let dispatch = match &self.engine {
            CameraEngineAccess::Running(running) => match send(running.handle()) {
                Ok(command) => CameraDispatch::Sent(command),
                Err(rejected) => CameraDispatch::Rejected(rejected),
            },
            CameraEngineAccess::Unavailable { .. } => CameraDispatch::EngineUnavailable,
        };
        self.counters.record(dispatch);
        if let CameraDispatch::Rejected(rejected) = dispatch {
            tracing::warn!(target: LOG_TARGET, %rejected, "команда жизненного цикла камеры отвергнута движком");
        }
        dispatch
    }
}

#[cfg(test)]
#[path = "camera_service_tests.rs"]
mod tests;
