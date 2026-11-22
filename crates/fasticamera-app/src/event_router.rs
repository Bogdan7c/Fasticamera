//! Маршрутизатор событий оболочки — единственный получатель очереди `ShellEvent`.
//!
//! Composition root: решений фич здесь нет, только раздача событий владельцам.
//! * Состояние экрана ([`DisplayState`]): на переднем плане ли приложение, текущая surface,
//!   ориентация. В сессии 02 маршрутизатор — временный владелец surface (рендера ещё нет):
//!   он держит её и сразу подтверждает освобождение. В сессии 03 это место займёт рендер.
//! * Тепловые показания → [`ThermalPolicy`] (единственный владелец тепловых решений, V8).
//! * Наблюдающий smoke-сценарий (если запущен) получает сводку каждого события и тики.
//!
//! Работает в своём потоке; главный поток Android его никогда не ждёт.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use shell_contract::commands::CommandOutcome;
use shell_contract::event::{ShellEvent, ShellEventKind};
use shell_contract::event_queue::{ReceiveError, ShellEventReceiver};
use shell_contract::motion::GyroRequest;
use shell_contract::thermal_policy::ThermalPolicy;

use render_loop::mailbox::InboxDelivery;

use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::{CameraScreenService, ScreenEnvironment};
use crate::camera_service::{AppVisibility, CameraDispatch, CameraServices};
use crate::capture_backend::CaptureWiring;
use crate::gpu_services::GpuServices;
use crate::jni_boundary::run_guarded;
use crate::logging::{LogPriority, emit_unfiltered};
use crate::render_route::RenderRoute;
use crate::result_echo::{EchoDue, ResultEcho};
use crate::result_line::{emit_stage_line, format_result_line};
use crate::scenario::observing::{
    DisplayState, ObservedEvent, ObservingScenario, RepeatOutcome, ScenarioContext, ScenarioOutput, ScenarioStep,
    ShellServices, StageLine,
};
use crate::scenario::{ScenarioConfig, ScenarioOutcome, ScenarioReport};

/// Шаг таймера наблюдающих сценариев. Это не порог поведения, а точность их часов:
/// сценарии меряют секунды, 50 мс точности им достаточно.
pub const SCENARIO_TICK: Duration = Duration::from_millis(50);

/// Пишет стадии и итоги сценариев в logcat.
#[derive(Debug, Default)]
pub struct LogcatScenarioOutput {
    /// Последняя выданная строка итога — для повторов (решение 58): logcat может её потерять.
    last_result_line: Option<String>,
}

impl ScenarioOutput for LogcatScenarioOutput {
    fn stage(&mut self, line: StageLine) {
        emit_stage_line(&line);
    }

    fn result(&mut self, report: ScenarioReport) {
        let (line, _json) = format_result_line(&report);
        emit_unfiltered(LogPriority::Info, line.clone());
        self.last_result_line = Some(line);
    }

    fn repeat_last_result(&mut self) -> RepeatOutcome {
        match &self.last_result_line {
            Some(line) => {
                emit_unfiltered(LogPriority::Info, line.clone());
                RepeatOutcome::Repeated
            }
            None => RepeatOutcome::NothingToRepeat,
        }
    }
}

/// Передаёт наблюдающие сценарии в поток маршрутизатора (из JNI-вызова `runScenario`).
#[derive(Clone)]
pub struct ScenarioStarter {
    sender: Sender<Box<dyn ObservingScenario>>,
}

impl ScenarioStarter {
    pub fn start(&self, scenario: Box<dyn ObservingScenario>) -> Result<(), String> {
        self.sender
            .send(scenario)
            .map_err(|_| "маршрутизатор событий не работает".to_owned())
    }
}

/// Что вошло в маршрутизатор.
pub struct RouterParts {
    pub events: ShellEventReceiver,
    pub services: ShellServices,
    pub thermal_policy: Arc<ThermalPolicy>,
    pub scenario_config: ScenarioConfig,
    pub gyro_request: GyroRequest,
    pub output: Box<dyn ScenarioOutput + Send>,
    /// Получатель surface и событий жизненного цикла.
    pub route: Box<dyn RenderRoute>,
    /// Что сценарии видят о GPU и рендере.
    pub gpu: GpuServices,
    /// Движок камеры и политика его жизненного цикла.
    pub camera: CameraServices,
    /// Экран камеры (сессия 12): включён ли и параметры его обвязки.
    pub camera_screen: CameraScreenConfig,
    /// Кто снимает для экрана: фейк по замыслу или готовый набор composition root (настоящая служба снимков,
    /// либо фейк-запасной с причиной).
    pub capture: CaptureWiring,
}

/// Итог одного шага маршрутизатора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterStep {
    Handled(ShellEventKind),
    Idle,
    /// Отправителей событий больше нет — маршрутизатору нечего делать.
    Stopped,
}

pub struct EventRouter {
    events: ShellEventReceiver,
    services: ShellServices,
    thermal_policy: Arc<ThermalPolicy>,
    scenario_config: ScenarioConfig,
    gyro_request: GyroRequest,
    output: Box<dyn ScenarioOutput + Send>,
    route: Box<dyn RenderRoute>,
    gpu: GpuServices,
    camera: CameraServices,
    /// Экран камеры: владелец установки/снятия и маршрутизации входа (`camera_screen_service`).
    camera_screen: CameraScreenService,
    display: DisplayState,
    arrivals: Receiver<Box<dyn ObservingScenario>>,
    active: Option<Box<dyn ObservingScenario>>,
    /// Повторы строки последнего итога (решение 58): logcat теряет строки во всплеске записи.
    result_echo: ResultEcho,
}

impl EventRouter {
    pub fn new(parts: RouterParts) -> (Self, ScenarioStarter) {
        let (sender, arrivals) = channel();
        let result_echo = ResultEcho::new(parts.scenario_config.result_echo);
        let router = Self {
            events: parts.events,
            services: parts.services,
            thermal_policy: parts.thermal_policy,
            scenario_config: parts.scenario_config,
            gyro_request: parts.gyro_request,
            output: parts.output,
            route: parts.route,
            gpu: parts.gpu,
            camera: parts.camera,
            camera_screen: {
                let capture = parts.capture.into_backend(&parts.camera_screen);
                CameraScreenService::with_capture(parts.camera_screen, capture)
            },
            display: DisplayState::default(),
            arrivals,
            active: None,
            result_echo,
        };
        (router, ScenarioStarter { sender })
    }

    /// Работает, пока живы отправители событий (на телефоне — весь процесс).
    pub fn run(mut self) {
        self.install_camera_screen();
        while self.step(SCENARIO_TICK) != RouterStep::Stopped {}
        tracing::info!(target: "fasticamera::router", "очередь событий закрыта, маршрутизатор остановлен");
    }

    /// Один шаг: принять новый сценарий, дождаться события (не дольше `wait`), дать тик.
    pub fn step(&mut self, wait: Duration) -> RouterStep {
        self.accept_new_scenario();
        let step = match self.events.recv_timeout(wait) {
            Ok(event) => {
                // Сценарий, запущенный до события, должен его увидеть (он мог прийти, пока ждали).
                self.accept_new_scenario();
                let observed = self.apply(event);
                self.drive_scenario(|scenario, context| scenario.on_event(observed, context));
                RouterStep::Handled(observed.kind)
            }
            Err(ReceiveError::Timeout) => RouterStep::Idle,
            Err(ReceiveError::SendersGone) => return RouterStep::Stopped,
        };
        self.drive_scenario(|scenario, context| scenario.on_tick(context));
        self.maintain_camera_screen(Instant::now());
        self.repeat_result_if_due(Instant::now());
        step
    }

    /// Очередная копия строки последнего итога, если по расписанию пора (решение 58).
    fn repeat_result_if_due(&mut self, now: Instant) {
        if self.result_echo.take_due(now) != EchoDue::Repeat {
            return;
        }
        if self.output.repeat_last_result() == RepeatOutcome::NothingToRepeat {
            // Расписание взводится только после выдачи итога, так что сюда попасть нельзя;
            // если всё же попали — расписание неверно, громко и без повторов дальше.
            tracing::warn!(target: "fasticamera::router", "повтор итога запрошен, но итога ещё не было");
            self.result_echo.cancel();
        }
    }

    pub fn display(&self) -> &DisplayState {
        &self.display
    }

    /// Диагностика экрана камеры (для сценариев и тестов).
    pub fn camera_screen(&self) -> &CameraScreenService {
        &self.camera_screen
    }

    /// Ставит экран камеры поверх видоискателя (при запуске маршрутизатора). Решений о содержимом экрана здесь
    /// нет: это забота сервиса экрана.
    pub(crate) fn install_camera_screen(&mut self) {
        let environment = ScreenEnvironment {
            services: &self.services,
            gpu: &self.gpu,
            camera: &self.camera,
            orientation: self.display.orientation(),
        };
        self.camera_screen.install(&environment);
    }

    /// Тик экрана камеры: досылка долга разрыва касаний и переустановка экрана, снятого рендером.
    fn maintain_camera_screen(&mut self, now: Instant) {
        let environment = ScreenEnvironment {
            services: &self.services,
            gpu: &self.gpu,
            camera: &self.camera,
            orientation: self.display.orientation(),
        };
        self.camera_screen.maintain(now, &environment);
    }

    /// Применяет событие к состоянию и возвращает его сводку для сценария.
    fn apply(&mut self, event: ShellEvent) -> ObservedEvent {
        let observed = ObservedEvent::of(&event);
        log_event(&event);
        match event {
            ShellEvent::SurfaceCreated(surface) => {
                self.display.surface_created(surface.generation());
                let delivery = self.route.surface_created(surface);
                log_undelivered(delivery, "surface_created");
            }
            ShellEvent::SurfaceChanged { generation, size } => {
                self.display.surface_changed(generation, size);
                let delivery = self.route.surface_changed(generation, size);
                log_undelivered(delivery, "surface_changed");
            }
            ShellEvent::SurfaceDestroyed(request) => {
                self.display.release_surface(request.generation());
                // Квитанцию подтвердит получатель после того, как реально отпустит surface.
                let delivery = self.route.surface_destroyed(request);
                log_undelivered(delivery, "surface_destroyed");
            }
            ShellEvent::Resumed => {
                self.display.set_resumed(true);
                let delivery = self.route.resumed();
                log_undelivered(delivery, "resumed");
                log_camera_dispatch(self.camera.app_resumed(), "resumed");
            }
            ShellEvent::Paused => {
                self.display.set_resumed(false);
                let delivery = self.route.paused();
                log_undelivered(delivery, "paused");
                log_camera_dispatch(self.camera.app_paused(), "paused");
            }
            ShellEvent::ThermalStatus(reading) => self.thermal_policy.observe(reading),
            ShellEvent::DeviceOrientation(orientation) => {
                self.display.set_orientation(orientation);
                self.camera_screen.route_orientation(orientation);
            }
            // Касания и клавиши идут экрану камеры (на поток рендера — через ограниченный канал, без ожиданий).
            ShellEvent::Touch(sample) => self.camera_screen.route_touch(sample),
            ShellEvent::TouchGap { .. } => self.camera_screen.route_touch_gap(),
            ShellEvent::HardwareKey(key) => self.camera_screen.route_hardware_key(key),
            ShellEvent::SystemGestureZones(zones) => self.camera_screen.route_system_gesture_zones(zones),
            // Разрешению и отказам платформы потребителей в 02 нет: только лог.
            ShellEvent::CameraPermission(_) | ShellEvent::PlatformCommandFailed(_) => {}
        }
        observed
    }

    fn accept_new_scenario(&mut self) {
        let scenario = match self.arrivals.try_recv() {
            Ok(scenario) => scenario,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
        };
        if let Some(previous) = self.active.take() {
            // Новый запуск вытесняет незавершённый — у старого честный итог, а не тишина.
            self.finish(
                previous.name(),
                ScenarioOutcome::Failed {
                    error: format!("сценарий вытеснен новым запуском ({})", scenario.name()),
                },
            );
        }
        // Экран камеры уступает место сценарию (если тот не просит его оставить); вернётся в `finish`.
        self.camera_screen
            .suspend_for_scenario(scenario.name(), scenario.camera_screen_policy(), &self.gpu);
        // Повторы прежнего итога (и только что выданного «вытеснен») больше не нужны: скрипт уже
        // ждёт новый сценарий, и копия чужого итога не должна ему попасться.
        self.result_echo.cancel();
        tracing::info!(target: "fasticamera::router", scenario = scenario.name(), "наблюдающий сценарий запущен");
        self.active = Some(scenario);
        self.drive_scenario(|scenario, context| scenario.start(context));
    }

    /// Вызывает активный сценарий с контекстом; завершившийся пишет итог и снимается.
    fn drive_scenario(
        &mut self,
        call: impl FnOnce(&mut dyn ObservingScenario, &mut ScenarioContext<'_>) -> ScenarioStep,
    ) {
        let Some(mut scenario) = self.active.take() else {
            return;
        };
        let name = scenario.name();
        let mut context = ScenarioContext::new(
            &self.services,
            &self.display,
            &self.thermal_policy,
            &self.scenario_config,
            &self.gpu,
            &self.camera,
            &self.camera_screen,
            self.gyro_request,
            Instant::now(),
            self.output.as_mut(),
            name,
        );
        // Паника сценария не должна убить маршрутизатор: тогда умерла бы вся обработка событий
        // (surface не подтверждалась бы, тепло не доходило бы до политики). Сценарий снимается
        // с честным итогом `panicked`, маршрутизатор работает дальше.
        match run_guarded(|| call(scenario.as_mut(), &mut context)) {
            Ok(ScenarioStep::Continue) => self.active = Some(scenario),
            Ok(ScenarioStep::Finished(outcome)) => self.finish(name, outcome),
            Err(panic) => {
                tracing::error!(target: "fasticamera::router", scenario = name, "сценарий запаниковал и снят");
                self.finish(name, ScenarioOutcome::Panicked { panic });
            }
        }
    }

    /// Любой конец сценария (успех, провал, вытеснение другим, паника, дедлайн) проходит здесь, поэтому
    /// именно здесь политика камеры возвращается к умолчанию, а камера, которую открывал сценарий,
    /// закрывается, а тронутые сценарием настройки рендера возвращаются. Делается ДО строки результата:
    /// скрипт, увидев результат, застаёт камеру и рендер в штатном состоянии.
    fn finish(&mut self, name: &'static str, outcome: ScenarioOutcome) {
        let visibility = if self.display.is_resumed() {
            AppVisibility::Foreground
        } else {
            AppVisibility::Background
        };
        let effects = self.camera.scenario_ended(visibility);
        if let Some(dispatch) = effects.close_claimed_camera {
            log_camera_dispatch(dispatch, "scenario_ended_close");
        }
        if let Some(dispatch) = effects.release_after_hold {
            log_camera_dispatch(dispatch, "scenario_ended_release_after_hold");
        }
        // Настройки рендера, которые сценарий менял (рисовальщик, стадии ISP, стенд, линия кадра,
        // частота экрана), возвращаются к штатным — тоже при любом конце сценария.
        let restored = self.gpu.render_scenario_ended(self.services.commands.as_ref());
        if !restored.is_empty() {
            tracing::info!(target: "fasticamera::router", scenario = name, ?restored, "настройки рендера возвращены");
        }
        // Экран камеры возвращается ПОСЛЕ возврата рендера: `render_scenario_ended` снял источник сценария (`glass`),
        // и порядок команд в ящике рендера — «снять источник сценария, поставить свежий экран».
        let environment = ScreenEnvironment {
            services: &self.services,
            gpu: &self.gpu,
            camera: &self.camera,
            orientation: self.display.orientation(),
        };
        self.camera_screen.resume_after_scenario(&environment);
        // Подсказка владельцу — состояние на экране, а не импульс: при любом конце сценария (в том числе
        // ошибке, таймауте, вытеснении и панике) она обязана исчезнуть, иначе «ЩЁЛКНИ ЛАМПОЙ» повисла бы навсегда.
        self.clear_operator_prompt(name);
        self.output.result(ScenarioReport {
            scenario: name.to_owned(),
            outcome,
        });
        self.result_echo.arm(Instant::now());
    }
}

impl EventRouter {
    /// Снимает подсказку владельцу. Отказ не молчит: переполненная очередь оставила бы подсказку на экране.
    fn clear_operator_prompt(&self, scenario: &'static str) {
        match self.services.operator_prompt.clear() {
            CommandOutcome::Queued => {}
            // Activity уже нет — снимать нечего.
            CommandOutcome::NotAttached => {
                tracing::debug!(target: "fasticamera::router", scenario, "подсказка не снята: Activity не подключена");
            }
            CommandOutcome::Backpressure => {
                tracing::warn!(target: "fasticamera::router", scenario, "очередь команд полна: подсказка владельцу могла остаться на экране");
            }
        }
    }
}

/// Поток рендера уже завершён: событие потеряно (для `surface_destroyed` квитанция уничтожена,
/// и оболочка сразу увидит `DroppedWithoutConfirmation`). Громко, но маршрутизатор живёт дальше.
fn log_undelivered(delivery: InboxDelivery, event: &'static str) {
    if delivery == InboxDelivery::ReceiverGone {
        tracing::error!(target: "fasticamera::router", event, "поток рендера не принимает события");
    }
}

/// Команда жизненного цикла не дошла до движка камеры: громко, но маршрутизатор живёт дальше.
/// Счётчики отказов ведёт сам сервис камеры.
fn log_camera_dispatch(dispatch: CameraDispatch, event: &'static str) {
    match dispatch {
        CameraDispatch::Sent(_) | CameraDispatch::KeptOpenByPolicy => {}
        CameraDispatch::EngineUnavailable => {
            tracing::debug!(target: "fasticamera::router", event, "движка камеры нет: команда не отправлена");
        }
        CameraDispatch::Rejected(rejected) => {
            tracing::error!(target: "fasticamera::router", event, %rejected, "движок камеры не принял команду");
        }
    }
}

fn log_event(event: &ShellEvent) {
    let kind = event.kind().name();
    match event {
        // Частые события — только на trace, чтобы не забивать буфер logcat (256 КиБ).
        ShellEvent::Touch(_) | ShellEvent::ThermalStatus(_) | ShellEvent::DeviceOrientation(_) => {
            tracing::trace!(target: "fasticamera::router", event = kind, "событие оболочки");
        }
        _ => tracing::debug!(target: "fasticamera::router", event = kind, "событие оболочки"),
    }
}

#[cfg(test)]
#[path = "event_router_tests.rs"]
mod tests;
