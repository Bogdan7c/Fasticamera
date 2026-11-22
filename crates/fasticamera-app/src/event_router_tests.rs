//! Функциональные тесты маршрутизатора: события идут по настоящей очереди `shell-contract`
//! через маршрутизатор до сценария и дальше до строки результата — на fake-оболочке.

use std::ffi::c_void;
use std::num::NonZeroU32;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use shell_contract::clock::BoottimeNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::diagnostics::{ShellCounters, ShellDiagnostics};
use shell_contract::event_queue::{ShellEventSender, ShellQueueConfig, shell_event_channel};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticRealizationKind, HapticsService};
use shell_contract::motion::{
    AngularVelocity, GyroSample, GyroSubscription, MotionRing, MotionSource, OrientationSubscription, SensorRate,
    SensorSourceError,
};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::permission::CameraPermission;
use shell_contract::power::{PowerSampling, PowerSamplingRequest, PowerSource, PowerSourceError};
use shell_contract::surface::NativeSurface;
use shell_contract::surface::{
    PlatformWindow, SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff,
};
use shell_contract::thermal::{
    HeadroomForecast, ThermalHeadroom, ThermalReading, ThermalSource, ThermalSourceError, ThermalStatus,
};
use shell_contract::thermal_policy::{ThermalPolicyConfig, ThermalPressure};

use render_loop::config::RenderLoopConfig;
use shell_contract::vsync::{VsyncSource, VsyncSourceError, VsyncSubscription};

use super::*;
use crate::camera_service::CameraServices;
use crate::camera_test_support::FakeCameraRig;
use crate::gpu_services::{GpuDiagnostics, GpuServices, RenderStatusSource, ValidationState};
use crate::gpu_test_support::{
    FakeGpuDiagnostics, FakeRenderStatus, ReceiptPolicy, RecordingRenderControl, RecordingRoute, RoutedEvent,
    presenting_status,
};
use crate::scenario::observing::{PlatformFacts, ThermalListenerReport};
use crate::scenario::{Scenario, ScenarioConfig};

#[path = "event_router_ae_trace_tests.rs"]
mod ae_trace;
#[path = "event_router_autostart_tests.rs"]
mod autostart;
#[path = "event_router_basic_scenarios_tests.rs"]
mod basic_scenarios;
#[path = "event_router_camera_tests.rs"]
mod camera;
#[path = "event_router_camera_scenario_tests.rs"]
mod camera_scenarios;
#[path = "event_router_camera_screen_tests.rs"]
mod camera_screen_e2e;
#[path = "event_router_echo_tests.rs"]
mod echo;
#[path = "event_router_raw_stream_tests.rs"]
mod raw_stream;
#[path = "event_router_render_tests.rs"]
mod render;
#[path = "event_router_three_a_tests.rs"]
mod three_a;
#[path = "event_router_ui_overlay_tests.rs"]
mod ui_overlay;
#[path = "event_router_viewfinder_tests.rs"]
mod viewfinder;

/// Fake всей оболочки: записывает команды и отдаёт заранее заданные ответы.
#[derive(Default)]
struct FakeShell {
    permission_requests: AtomicU64,
    frame_rates: Mutex<Vec<f32>>,
    haptics_played: Mutex<Vec<HapticPattern>>,
    /// Что отвечает хаптика; `None` — «поставлено в очередь» (обычный телефон с вибромотором).
    haptics_outcome: Mutex<Option<HapticOutcome>>,
    /// Экран подсказок владельцу: что показано и снято, с настраиваемым ответом платформы.
    prompts: Arc<crate::operator_prompt_fake::FakePrompts>,
    counters: Mutex<ShellCounters>,
    gyro_samples: Mutex<Vec<GyroSample>>,
    /// Ящик гравитации: тест кладёт показания, подписка `start_gravity` отдаёт именно его.
    gravity: Arc<shell_contract::gravity::GravitySlot>,
    /// Подставная батарея; `None` — батареи нет (честный отказ замера тока).
    battery: Mutex<Option<Arc<shell_contract::power::fake::FakePowerSource>>>,
    /// Датчик ориентации: кто и сколько его держит.
    orientation_sensor: crate::orientation_sensor_fake::FakeOrientationSensor,
}

impl ShellCommands for FakeShell {
    fn request_camera_permission(&self) -> CommandOutcome {
        self.permission_requests.fetch_add(1, Ordering::SeqCst);
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, rate: FrameRate) -> FrameRateOutcome {
        self.frame_rates.lock().expect("замок").push(rate.hertz());
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

impl HapticsService for FakeShell {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        self.haptics_played.lock().expect("замок").push(pattern);
        (*self.haptics_outcome.lock().expect("замок")).unwrap_or(HapticOutcome::Queued(HapticRealizationKind::Waveform))
    }
}

impl MotionSource for FakeShell {
    fn start_gyro(&self, request: shell_contract::motion::GyroRequest) -> Result<GyroSubscription, SensorSourceError> {
        let ring = Arc::new(MotionRing::new(request.ring_capacity()));
        for sample in self.gyro_samples.lock().expect("замок").iter() {
            ring.push(*sample);
        }
        Ok(GyroSubscription::new(ring, Box::new(())))
    }

    fn start_orientation(&self) -> Result<OrientationSubscription, SensorSourceError> {
        Ok(self.orientation_sensor.subscribe())
    }

    fn start_gravity(&self) -> Result<shell_contract::gravity::GravitySubscription, SensorSourceError> {
        Ok(
            shell_contract::gravity::GravitySubscription::new(Arc::clone(&self.gravity), Box::new(()))
                .with_source(shell_contract::gravity::GravitySource::Injected),
        )
    }
}

impl ThermalSource for FakeShell {
    fn current_status(&self) -> Result<ThermalStatus, ThermalSourceError> {
        Ok(ThermalStatus::None)
    }

    fn headroom(&self, _forecast: HeadroomForecast) -> ThermalHeadroom {
        ThermalHeadroom::Measured(0.4)
    }
}

impl ShellDiagnostics for FakeShell {
    fn counters(&self) -> ShellCounters {
        *self.counters.lock().expect("замок")
    }
}

impl VsyncSource for FakeShell {
    fn start_vsync(&self) -> Result<VsyncSubscription, VsyncSourceError> {
        Err(VsyncSourceError::NoLooper)
    }
}

/// Без подключённой батареи сценарии, которым нужен ток, видят честный отказ.
impl PowerSource for FakeShell {
    fn start_sampling(&self, request: PowerSamplingRequest) -> Result<PowerSampling, PowerSourceError> {
        match self.battery.lock().expect("замок").as_ref() {
            Some(battery) => battery.start_sampling(request),
            None => Err(PowerSourceError::Unavailable {
                reason: "fake-оболочка без батареи".to_owned(),
            }),
        }
    }
}

impl PlatformFacts for FakeShell {
    fn thermal_listener(&self) -> ThermalListenerReport {
        ThermalListenerReport::Registered
    }

    fn has_vibrator(&self) -> bool {
        true
    }
}

/// Собирает стадии и итоги сценариев.
#[derive(Clone, Default)]
struct CollectedOutput {
    stages: Arc<Mutex<Vec<serde_json::Value>>>,
    results: Arc<Mutex<Vec<serde_json::Value>>>,
    /// Повторы итога (решение 58) — отдельно от `results`: «ровно один итог» остаётся проверкой.
    repeats: Arc<Mutex<Vec<(Instant, serde_json::Value)>>>,
}

impl ScenarioOutput for CollectedOutput {
    fn stage(&mut self, line: StageLine) {
        self.stages
            .lock()
            .expect("замок")
            .push(serde_json::to_value(line).expect("json"));
    }

    fn result(&mut self, report: ScenarioReport) {
        self.results
            .lock()
            .expect("замок")
            .push(serde_json::to_value(report).expect("json"));
    }

    fn repeat_last_result(&mut self) -> RepeatOutcome {
        let last = self.results.lock().expect("замок").last().cloned();
        match last {
            Some(result) => {
                self.repeats.lock().expect("замок").push((Instant::now(), result));
                RepeatOutcome::Repeated
            }
            None => RepeatOutcome::NothingToRepeat,
        }
    }
}

impl CollectedOutput {
    fn stage_names(&self) -> Vec<String> {
        self.stages
            .lock()
            .expect("замок")
            .iter()
            .map(|stage| stage["stage"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn single_result(&self) -> serde_json::Value {
        let results = self.results.lock().expect("замок");
        assert_eq!(results.len(), 1, "ожидался один итог: {results:?}");
        results[0].clone()
    }
}

struct FakeWindow;

impl PlatformWindow for FakeWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

struct Harness {
    router: EventRouter,
    starter: ScenarioStarter,
    sender: ShellEventSender,
    shell: Arc<FakeShell>,
    output: CollectedOutput,
    policy: Arc<ThermalPolicy>,
    /// Что получил маршрут событий экрана.
    route_log: Arc<Mutex<Vec<RoutedEvent>>>,
    /// Surface, которые сейчас держит получатель (не маршрутизатор).
    route_surfaces: Arc<Mutex<Vec<NativeSurface>>>,
    render: Arc<FakeRenderStatus>,
    /// Регулировки рендера, которые слали сценарии и маршрутизатор.
    control: Arc<RecordingRenderControl>,
    /// Сервис камеры, который видит маршрутизатор.
    camera: CameraServices,
    /// Движок камеры на fake-бэкенде (настоящий поток, подменена платформа); `None`, если сервис задан снаружи.
    rig: Option<FakeCameraRig>,
}

impl Harness {
    /// Fake-бэкенд камеры и диагностика (в тестах с настоящим движком).
    fn rig(&self) -> &FakeCameraRig {
        self.rig.as_ref().expect("в этом тесте движок камеры fake")
    }
}

fn harness(config: ScenarioConfig) -> Harness {
    harness_with_render(config, RenderLoopConfig::default(), ReceiptPolicy::Confirm)
}

/// Маршрутизатор с движком камеры на fake-бэкенде (настройки сценариев камеры — у `rig`).
fn harness_for_rig(rig: FakeCameraRig) -> Harness {
    let camera = rig.services.clone();
    let mut harness = harness_for_services(camera);
    harness.rig = Some(rig);
    harness
}

/// Маршрутизатор с заданным сервисом камеры (например, `CameraServices::unavailable`).
fn harness_for_services(camera: CameraServices) -> Harness {
    build_harness_with_camera(
        ScenarioConfig::default(),
        Box::new(RecordingRoute::new(ReceiptPolicy::Confirm)),
        FakeRenderStatus::live(presenting_status(), ValidationState::Enabled),
        FakeGpuDiagnostics::new(Vec::new(), None),
        RenderLoopConfig::default(),
        camera,
    )
}

/// Маршрутизатор с подставными рендером и GPU-диагностиками.
fn harness_with_render(config: ScenarioConfig, render_config: RenderLoopConfig, receipts: ReceiptPolicy) -> Harness {
    let route = RecordingRoute::new(receipts);
    let route_log = Arc::clone(&route.log);
    let route_surfaces = Arc::clone(&route.held_surfaces);
    let render = FakeRenderStatus::live(presenting_status(), ValidationState::Enabled);
    let diagnostics = FakeGpuDiagnostics::new(Vec::new(), None);
    let mut harness = build_harness(config, Box::new(route), render, diagnostics, render_config);
    harness.route_log = route_log;
    harness.route_surfaces = route_surfaces;
    harness
}

/// Сборка с готовым маршрутом и GPU-сервисами.
fn build_harness(
    config: ScenarioConfig,
    route: Box<dyn RenderRoute>,
    render: Arc<FakeRenderStatus>,
    diagnostics: Arc<FakeGpuDiagnostics>,
    render_config: RenderLoopConfig,
) -> Harness {
    let rig = FakeCameraRig::new();
    let camera = rig.services.clone();
    let mut harness = build_harness_with_camera(config, route, render, diagnostics, render_config, camera);
    harness.rig = Some(rig);
    harness
}

/// Сборка с готовым маршрутом, GPU-сервисами и движком камеры.
fn build_harness_with_camera(
    config: ScenarioConfig,
    route: Box<dyn RenderRoute>,
    render: Arc<FakeRenderStatus>,
    diagnostics: Arc<FakeGpuDiagnostics>,
    render_config: RenderLoopConfig,
    camera: CameraServices,
) -> Harness {
    let (sender, events) = shell_event_channel(ShellQueueConfig::default()).expect("очередь");
    let shell = Arc::new(FakeShell::default());
    let output = CollectedOutput::default();
    let control = RecordingRenderControl::new();
    let policy = Arc::new(ThermalPolicy::new(ThermalPolicyConfig::default()).expect("политика"));
    let (router, starter) = EventRouter::new(RouterParts {
        events,
        services: ShellServices {
            commands: shell.clone(),
            haptics: shell.clone(),
            operator_prompt: shell.prompts.clone(),
            motion: shell.clone(),
            thermal: shell.clone(),
            diagnostics: shell.clone(),
            vsync: shell.clone(),
            facts: shell.clone(),
            power: shell.clone(),
        },
        thermal_policy: Arc::clone(&policy),
        scenario_config: config,
        gyro_request: shell_contract::motion::GyroRequest {
            rate: SensorRate::new(NonZeroU32::new(400).expect("частота")),
            retained: Duration::from_secs(5),
        },
        output: Box::new(output.clone()),
        route,
        gpu: GpuServices::new(
            Arc::clone(&render) as Arc<dyn RenderStatusSource>,
            Arc::clone(&diagnostics) as Arc<dyn GpuDiagnostics>,
            render_config,
            Arc::clone(&control) as Arc<dyn crate::viewfinder_control::RenderControl>,
        ),
        camera: camera.clone(),
        // Экран камеры выключен: прежние тесты проверяют сценарии без него (экрану — event_router_camera_screen_tests).
        camera_screen: crate::camera_screen_config::CameraScreenConfig {
            enabled: false,
            ..Default::default()
        },
        capture: crate::capture_backend::CaptureWiring::Fake,
    });
    Harness {
        router,
        starter,
        sender,
        shell,
        output,
        policy,
        route_log: Arc::default(),
        route_surfaces: Arc::default(),
        render,
        control,
        camera,
        rig: None,
    }
}

impl Harness {
    fn start(&self, scenario: Scenario) {
        self.starter
            .start(scenario.observing().expect("наблюдающий сценарий"))
            .expect("старт");
    }

    /// Отправляет событие и даёт маршрутизатору его обработать.
    fn deliver(&mut self, event: ShellEvent) {
        self.sender.send(event);
        assert!(matches!(
            self.router.step(Duration::from_secs(1)),
            RouterStep::Handled(_)
        ));
    }

    /// Новая surface с номером `generation`.
    fn surface_created(&mut self, generation: u64) {
        self.deliver(ShellEvent::SurfaceCreated(NativeSurface::new(
            SurfaceGeneration::new(generation),
            Box::new(FakeWindow),
        )));
    }

    /// Размер surface (Android присылает его сразу после создания).
    fn surface_changed(&mut self, generation: u64) {
        self.deliver(ShellEvent::SurfaceChanged {
            generation: SurfaceGeneration::new(generation),
            size: SurfaceSize {
                width_px: 1440,
                height_px: 3216,
            },
        });
    }

    /// Уничтожение surface так, как его делает оболочка: квитанция + ожидание подтверждения.
    fn surface_destroyed(&mut self, generation: u64) -> SurfaceReleaseOutcome {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        self.deliver(ShellEvent::SurfaceDestroyed(request));
        waiter.wait(Duration::from_secs(1))
    }

    /// Крутит маршрутизатор без событий, пока сценарий не напишет итог (или 5 с).
    fn run_until_result(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
            self.router.step(Duration::from_millis(2));
        }
    }
}
