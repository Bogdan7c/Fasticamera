//! Движок камеры: детерминированная машина состояний без потоков.
//!
//! Движок владеет бэкендом, **желаемым** состоянием ([`Desired`]: какая камера, режим, идёт
//! ли поток, параметры), **реальным** состоянием ([`CameraState`], открытое устройство
//! и сессия), каталогом, статистикой темпа, счётчиками и исходящими событиями. Команды
//! меняют только желаемое; [`CameraEngine::reconcile`] доводит до него реальное синхронными
//! вызовами бэкенда: `open → configure → set_repeating` (решение 39). После вытеснения или
//! сбоя то же доведение выполняется снова — по сигналу «камера свободна», по таймеру или при
//! возврате на передний план.
//!
//! Время — параметр (`now: Instant`): внутри нет `Instant::now()`, поэтому тесты управляют
//! часами. Потоки, очереди и паники — забота [`crate::runtime`]; движок тестируется напрямую.
//!
//! Раскладка:
//! * `commands` — желаемое состояние и валидация команд;
//! * `reconcile` — доведение реального состояния до желаемого;
//! * `recovery` — ошибки, планы восстановления, попытки, таймер;
//! * `backend_events` — события бэкенда (поколения, stale, темп);
//! * `three_a` — автоматика 3A: тап и блокировка, ручные параметры, серии
//!   (исполнители подключаются через [`crate::ThreeAWiring`]);
//! * `white` — владелец белой: единственная действующая белая, заявки источнику, сроки, память
//!   (живёт вне `Desired`).
//!
//! Решения, не заданные заданием:
//! * `EnterForeground` выводит из `Fatal` только `RecoveryExhausted` (свежая попытка пользователя)
//!   и `CatalogUnavailable` (повторное чтение каталога); `BackendUnavailable` и `EnginePanicked`
//!   окончательны;
//! * повторный `open` той же камеры в `Error::Recoverable` — `AlreadyInState`: повтор запускают
//!   сигнал свободы камеры, таймер или `EnterForeground`, а не команда;
//! * `Opened` значит «устройство открыто, повторяющегося запроса нет»; настроена ли сессия —
//!   видно в [`CameraStatus::configured`];
//! * неожиданное `SessionClosed` текущей сессии — признак проблемы устройства: идёт тем же путём,
//!   что сбой устройства (`DeviceFailure`, повтор по таймеру с нарастающей задержкой);
//! * счётчик подряд идущих попыток обнуляется не при возврате в целевое состояние, а после
//!   `recovery.stable_frames_before_reset` результатов кадров подряд (иначе платформа, закрывающая
//!   сессию сразу после каждой настройки, держала бы движок в бесконечном цикле).

mod availability_memory;
mod backend_events;
mod camera_frame_rate;
mod commands;
mod flash;
mod frame_rate_fallback;
mod lens_intent;
mod lens_switch;
mod reconcile;
mod recovery;
mod standby;
mod three_a;
mod white;

use std::sync::Arc;
use std::time::Instant;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::catalog::CameraCatalog;
use camera_contract::control::CaptureIntent;
use camera_contract::ids::CameraId;
use camera_contract::stream::ConfiguredStream;

use crate::backend::{BackendEvent, BackendFailure, CameraBackend, DeviceGeneration, SessionGeneration};
use crate::cadence::CadenceTracker;
use crate::command::{CameraCommand, CommandId};
use crate::config::CameraEngineConfig;
use crate::event::CameraEvent;
use crate::lens_wiring::LensWiring;
use crate::request_controls::RequestControls;
use crate::role_classifier::{RoleDiagnostic, classify_catalog};
use crate::state::{CameraError, CameraState, ClosedReason, FatalCause};
use crate::status::{AppPresence, CameraCounters, CameraStatus};
use crate::three_a_wiring::ThreeAWiring;
use crate::zoom::{ZoomDiagnostic, build_zoom_catalog_reporting};
use availability_memory::AvailabilityMemory;
use camera_frame_rate::CameraFrameRateLimits;
pub(crate) use commands::Desired;
use flash::FlashEngine;
use lens_intent::LensEngineState;
use recovery::RecoveryTracker;
use standby::StandbyEngine;
use three_a::ThreeAEngine;
use white::WhiteEngine;
pub(crate) use white::initial_white_status;

pub(crate) const LOG_TARGET: &str = "fasticamera::camera::engine";

/// Открытое устройство: какая камера и под каким поколением.
#[derive(Debug, Clone)]
struct OpenDevice {
    camera: CameraId,
    generation: DeviceGeneration,
}

/// Параметры идущего повторяющегося запроса.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RepeatingRequest {
    intent: CaptureIntent,
    /// План управляющей части, построенный из намерения 3A и возможностей камеры.
    controls: RequestControls,
}

/// Сконфигурированная сессия.
#[derive(Debug, Clone)]
struct LiveSession {
    generation: SessionGeneration,
    configured: ConfiguredStream,
    repeating: Option<RepeatingRequest>,
}

/// Шаг доведения остановлен (ошибка или ожидание): дальше идти нельзя. Состояние ошибки
/// к этому моменту уже записано.
#[derive(Debug, Clone, Copy)]
struct Halt;

/// Бэкенд-пустышка для движка, у которого настоящий бэкенд не создался. Не вызывается:
/// `Fatal` закрывает все пути к бэкенду.
struct DeadBackend;

impl DeadBackend {
    fn failure(operation: &'static str) -> BackendFailure {
        BackendFailure {
            operation,
            code: -1,
            detail: "бэкенд камеры не создан".to_owned(),
        }
    }
}

impl CameraBackend for DeadBackend {
    fn load_catalog(&mut self) -> Result<Vec<camera_contract::capabilities::CameraDescriptor>, BackendFailure> {
        Err(Self::failure("load_catalog"))
    }

    fn open_device(
        &mut self,
        _camera: &CameraId,
        _generation: DeviceGeneration,
    ) -> Result<(), crate::backend::OpenFailure> {
        Err(crate::backend::OpenFailure::Backend(Self::failure("open_device")))
    }

    fn configure_session(
        &mut self,
        _stream: &camera_contract::stream::ResolvedStream,
        _generation: SessionGeneration,
    ) -> Result<ConfiguredStream, crate::backend::ConfigureFailure> {
        Err(crate::backend::ConfigureFailure::Backend(Self::failure(
            "configure_session",
        )))
    }

    fn set_repeating_request(
        &mut self,
        _intent: CaptureIntent,
        _controls: &RequestControls,
    ) -> Result<(), crate::backend::RequestFailure> {
        Err(crate::backend::RequestFailure::Backend(Self::failure(
            "set_repeating_request",
        )))
    }

    fn submit_focus_trigger(
        &mut self,
        _intent: CaptureIntent,
        _controls: &RequestControls,
        _trigger: crate::request_controls::AfTrigger,
        _request: camera_contract::three_a::PointRequestId,
    ) -> Result<(), crate::backend::RequestFailure> {
        Err(crate::backend::RequestFailure::Backend(Self::failure(
            "submit_focus_trigger",
        )))
    }

    fn submit_bracket(
        &mut self,
        _intent: CaptureIntent,
        _base: &RequestControls,
        _bracket: &camera_contract::three_a::ExposureBracket,
        _burst: camera_contract::three_a::BurstId,
    ) -> Result<(), crate::backend::RequestFailure> {
        Err(crate::backend::RequestFailure::Backend(Self::failure("submit_bracket")))
    }

    fn submit_flash_still(
        &mut self,
        _intent: CaptureIntent,
        _controls: &RequestControls,
        _shot: camera_contract::flash::FlashShotId,
    ) -> Result<(), crate::backend::RequestFailure> {
        Err(crate::backend::RequestFailure::Backend(Self::failure(
            "submit_flash_still",
        )))
    }

    fn stop_repeating(&mut self) -> Result<(), crate::backend::RequestFailure> {
        Ok(())
    }

    fn close_session(&mut self) {}

    fn close_device(&mut self) {}
}

/// Движок камеры. Все входы — методы `&mut self` с явным `now`.
pub struct CameraEngine {
    backend: Box<dyn CameraBackend>,
    config: CameraEngineConfig,
    desired: Desired,
    presence: AppPresence,
    catalog: Arc<CameraCatalog>,
    catalog_loaded: bool,
    role_diagnostics: Vec<RoleDiagnostic>,
    zoom_diagnostics: Vec<ZoomDiagnostic>,
    state: CameraState,
    device: Option<OpenDevice>,
    session: Option<LiveSession>,
    /// Идёт перестройка работавшего потока: промежуточные `Opening/Opened/Configuring`
    /// не показываются, пока состояние `Reconfiguring`.
    reconfiguring: bool,
    recovery: RecoveryTracker,
    /// Последняя известная доступность камер (см. `availability_memory`).
    availability: AvailabilityMemory,
    cadence: CadenceTracker,
    /// Потолки частоты по камерам: желаемый режим не меняется, меняется эффективный.
    frame_rate_limits: CameraFrameRateLimits,
    counters: CameraCounters,
    /// Автоматика 3A: исполнители, таймеры, серия (желаемое намерение — в `desired.three_a`).
    three_a: ThreeAEngine,
    /// Владелец белой: вне `Desired`, команды `Close`/`ApplyControls`/`SwitchTo` его не трогают.
    white: WhiteEngine,
    /// Снимок со вспышкой: последовательность, счётчики, наблюдатель (14d).
    flash: FlashEngine,
    /// Шкала зума, чтение намерений экрана и учёт смены объектива (13).
    lens: LensEngineState,
    /// «Тёплые» соседние камеры (13b).
    standby: StandbyEngine,
    last_unavailable: Option<CameraUnavailableReason>,
    events: Vec<CameraEvent>,
    /// После паники внутри итерации бэкенд и внутренности в неизвестном состоянии:
    /// движок только отвечает на команды `EngineFailed`.
    poisoned: bool,
}

impl CameraEngine {
    /// Движок с рабочим бэкендом; каталог читается отдельно ([`CameraEngine::load_catalog`]).
    pub fn new(config: CameraEngineConfig, presence: AppPresence, backend: Box<dyn CameraBackend>) -> Self {
        let wiring = ThreeAWiring::vendor(&config.three_a);
        Self::with_wiring(config, presence, backend, wiring)
    }

    /// Движок с заданными исполнителями 3A и подключениями белой (корень композиции, 08a, тесты).
    pub fn with_wiring(
        config: CameraEngineConfig,
        presence: AppPresence,
        backend: Box<dyn CameraBackend>,
        wiring: ThreeAWiring,
    ) -> Self {
        let state = CameraState::Closed {
            reason: match presence {
                AppPresence::Foreground => ClosedReason::NotRequested,
                AppPresence::Background => ClosedReason::AppInBackground,
            },
        };
        let desired = Desired::default();
        let ThreeAWiring {
            exposure,
            focus,
            white,
            exposure_directives,
            flash_observer,
        } = wiring;
        let flash = FlashEngine::new(flash_observer);
        let white = WhiteEngine::new(config.three_a.white, white);
        let mut engine = Self {
            cadence: CadenceTracker::new(config.cadence),
            three_a: ThreeAEngine::new(exposure, focus, exposure_directives, &desired.three_a),
            white,
            flash,
            backend,
            config,
            desired,
            presence,
            catalog: Arc::new(CameraCatalog::empty()),
            catalog_loaded: false,
            role_diagnostics: Vec::new(),
            zoom_diagnostics: Vec::new(),
            state,
            device: None,
            session: None,
            reconfiguring: false,
            recovery: RecoveryTracker::default(),
            availability: AvailabilityMemory::default(),
            frame_rate_limits: CameraFrameRateLimits::default(),
            counters: CameraCounters::default(),
            lens: LensEngineState::new(),
            standby: StandbyEngine::new(),
            last_unavailable: None,
            events: Vec::new(),
            poisoned: false,
        };
        // Стартовая белая (из памяти или запасная) и её прижим к пределам видны с первого статуса.
        engine.flush_white_events();
        engine
    }

    /// Движок с подключённым ящиком намерений объектива (корень композиции, `CameraRuntime`, тесты).
    pub fn with_lens_wiring(mut self, wiring: LensWiring) -> Self {
        self.lens.attach_intents(wiring.intents);
        self
    }

    /// Движок, у которого бэкенд не создался на потоке камеры: сразу `Fatal::BackendUnavailable`,
    /// команды отвечают `EngineFailed`.
    pub fn failed_backend(config: CameraEngineConfig, presence: AppPresence, failure: BackendFailure) -> Self {
        let mut engine = Self::new(config, presence, Box::new(DeadBackend));
        tracing::error!(target: LOG_TARGET, %failure, "бэкенд камеры не создан");
        engine.enter_fatal(FatalCause::BackendUnavailable { failure });
        engine
    }

    /// Читает каталог камер и назначает роли. Сбой — `Fatal::CatalogUnavailable`.
    pub fn load_catalog(&mut self, now: Instant) {
        let failure = match self.backend.load_catalog() {
            Err(failure) => failure,
            Ok(cameras) => match classify_catalog(cameras, &self.config.roles) {
                Ok(classified) => {
                    for diagnostic in &classified.diagnostics {
                        tracing::warn!(target: LOG_TARGET, ?diagnostic, "роль камеры назначена не полностью");
                    }
                    self.role_diagnostics = classified.diagnostics;
                    let zoom = build_zoom_catalog_reporting(&classified.catalog, &self.config.zoom);
                    for diagnostic in &zoom.diagnostics {
                        tracing::warn!(target: LOG_TARGET, ?diagnostic, "шкала зума построена не полностью");
                    }
                    self.zoom_diagnostics = zoom.diagnostics;
                    self.lens.set_zoom_catalog(Arc::new(zoom.catalog));
                    self.catalog = Arc::new(classified.catalog);
                    self.catalog_loaded = true;
                    // Экран мог опубликовать намерение раньше, чем прочитался каталог.
                    self.refresh_lens_intent(now);
                    return;
                }
                Err(error) => BackendFailure {
                    operation: "classify_catalog",
                    code: 0,
                    detail: error.to_string(),
                },
            },
        };
        tracing::error!(target: LOG_TARGET, %failure, "каталог камер не прочитан");
        self.enter_fatal(FatalCause::CatalogUnavailable { failure });
    }

    /// Команда снаружи. Всегда ровно одно событие [`CameraEvent::CommandHandled`], причём
    /// до событий, которые команда вызвала.
    pub fn handle_command(&mut self, id: CommandId, command: CameraCommand, now: Instant) {
        self.counters.commands_handled += 1;
        let name = command.name();
        let outcome = self.decide_command(id, &command, now);
        self.events.push(CameraEvent::CommandHandled {
            command: id,
            name,
            outcome: outcome.clone(),
        });
        // Отклонённая команда «взять белую» оставляет экрану отчёт «не спрашивали» (в том числе в `Fatal`).
        self.note_white_take_outcome(id, &command, &outcome);
        // Команда могла поставить или снять блокировку AE/AF: слежению за белой она нужна сразу.
        self.sync_white_lock();
        // События белой, вызванные решением команды, идут строго после её итога.
        self.flush_white_events();
        // Итог снимка со вспышкой идёт строго после ответа на команду.
        self.settle_flash_command(&command, &outcome, now);
        if self.poisoned {
            return;
        }
        self.apply_command_effects(&command, &outcome, now);
        self.flush_white_events();
        self.three_a.publish_exposure_intent(&self.desired.three_a);
    }

    /// Асинхронное событие бэкенда.
    pub fn handle_backend_event(&mut self, event: BackendEvent, now: Instant) {
        if self.poisoned {
            return;
        }
        self.route_backend_event(event, now);
        self.flush_white_events();
    }

    /// Сработал таймер: пора ли повторять попытку восстановления.
    pub fn on_timer(&mut self, now: Instant) {
        if self.poisoned {
            return;
        }
        self.fire_retry_timer_if_due(now);
        self.fire_three_a_timers(now);
        self.fire_white_deadline(now);
        self.flush_white_events();
        self.fire_standby_release_timer(now);
    }

    /// Статус 3A: вспышке нужны возможности камеры и состояние потока, поэтому её блок собирается здесь.
    fn three_a_status(&self) -> crate::three_a_status::ThreeAStatus {
        crate::three_a_status::ThreeAStatus {
            flash: self.flash_status(),
            white: self.white.status(),
            ..self.three_a.status(&self.desired.three_a)
        }
    }

    /// Когда движку нужно проснуться без внешних событий.
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.poisoned {
            return None;
        }
        [
            self.pending_retry_deadline(),
            self.three_a.next_deadline(),
            self.white.next_deadline(),
            self.standby.next_deadline(),
            self.flash.deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Снимок состояния.
    pub fn status(&self) -> CameraStatus {
        CameraStatus {
            state: self.state.clone(),
            presence: self.presence,
            catalog: Arc::clone(&self.catalog),
            configured: self.session.as_ref().map(|session| session.configured.clone()),
            stream: self.cadence.stats().clone(),
            last_unavailable: self.last_unavailable,
            counters: self.counters,
            three_a: self.three_a_status(),
            lens: self.lens_status(),
        }
    }

    /// Забрать накопленные события (в порядке появления).
    pub fn drain_events(&mut self) -> Vec<CameraEvent> {
        std::mem::take(&mut self.events)
    }

    /// Диагностика построения шкалы зума последнего прочитанного каталога (исключённые камеры).
    pub fn zoom_diagnostics(&self) -> &[ZoomDiagnostic] {
        &self.zoom_diagnostics
    }

    /// Диагностика назначения ролей последнего прочитанного каталога.
    pub fn role_diagnostics(&self) -> &[RoleDiagnostic] {
        &self.role_diagnostics
    }

    /// Итерация запаниковала: движок окончательно в `Fatal::EnginePanicked`, устройство
    /// закрывается (сама попытка закрытия тоже под перехватом паники).
    pub fn fail_after_panic(&mut self, message: String) {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        tracing::error!(target: LOG_TARGET, %message, "паника в итерации движка камеры");
        self.poisoned = true;
        if catch_unwind(AssertUnwindSafe(|| self.backend.close_device())).is_err() {
            tracing::error!(target: LOG_TARGET, "паника и при закрытии устройства после паники движка");
        }
        if catch_unwind(AssertUnwindSafe(|| self.close_standby_resources())).is_err() {
            tracing::error!(target: LOG_TARGET, "паника и при закрытии тёплых камер после паники движка");
        }
        self.device = None;
        self.session = None;
        self.end_three_a_stream();
        self.reconfiguring = false;
        self.recovery.stop_waiting();
        self.transition(CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::EnginePanicked { message },
            },
        });
    }

    /// Если после паники в итерации команда осталась без итога — ответить `EngineFailed`.
    /// Так «ровно одно `CommandHandled` на команду» держится и при панике до публикации итога.
    pub fn answer_if_unanswered(&mut self, id: CommandId, name: &'static str) {
        let answered = self
            .events
            .iter()
            .any(|event| matches!(event, CameraEvent::CommandHandled { command, .. } if *command == id));
        if !answered {
            self.events.push(CameraEvent::CommandHandled {
                command: id,
                name,
                outcome: crate::command::CommandOutcome::InvalidState(crate::command::InvalidStateReason::EngineFailed),
            });
        }
    }

    /// Остановка потока: отпустить устройство. Состояние `Fatal` остаётся как есть.
    pub fn shutdown(&mut self) {
        if !self.poisoned {
            self.release_platform();
            self.standby.end_demand();
        }
        self.recovery.stop_waiting();
        if !self.is_fatal() {
            self.transition(CameraState::Closed {
                reason: ClosedReason::NotRequested,
            });
        }
    }

    // ---- Общие помощники для подмодулей ----

    pub(crate) fn is_fatal(&self) -> bool {
        matches!(
            self.state,
            CameraState::Error {
                error: CameraError::Fatal { .. }
            }
        )
    }

    /// Сменить состояние; одинаковое состояние события не порождает.
    fn transition(&mut self, new_state: CameraState) {
        if self.state == new_state {
            return;
        }
        tracing::debug!(target: LOG_TARGET, from = self.state.name(), to = new_state.name(), "смена состояния");
        self.state = new_state.clone();
        self.events.push(CameraEvent::StateChanged { state: new_state });
    }

    /// Закрыть устройство на бэкенде, если оно у нас записано открытым.
    fn release_platform(&mut self) {
        // Тёплые закрываются раньше основной: основная закрыта — тёплым рядом делать нечего.
        self.close_standby_resources();
        if self.device.is_some() || self.session.is_some() {
            self.backend.close_device();
        }
        self.device = None;
        self.session = None;
        self.end_three_a_stream();
    }

    /// Окончательная ошибка: устройство отпущено, ожиданий нет.
    fn enter_fatal(&mut self, cause: FatalCause) {
        self.release_platform();
        self.standby.end_demand();
        self.reset_lens_switch();
        self.reconfiguring = false;
        self.recovery.stop_waiting();
        self.transition(CameraState::Error {
            error: CameraError::Fatal { cause },
        });
    }
}

#[cfg(test)]
mod tests;
