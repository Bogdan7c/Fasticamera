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
use crate::gpu_services::{GpuDiagnostics, GpuServices, RenderStatusSource, ValidationState};
use crate::gpu_test_support::{
    FakeGpuDiagnostics, FakeRenderStatus, ReceiptPolicy, RecordingRoute, RoutedEvent, presenting_status,
};
use crate::scenario::observing::{PlatformFacts, ThermalListenerReport};
use crate::scenario::{Scenario, ScenarioConfig};

#[path = "event_router_render_tests.rs"]
mod render;

/// Fake всей оболочки: записывает команды и отдаёт заранее заданные ответы.
#[derive(Default)]
struct FakeShell {
    permission_requests: AtomicU64,
    frame_rates: Mutex<Vec<f32>>,
    haptics_played: Mutex<Vec<HapticPattern>>,
    counters: Mutex<ShellCounters>,
    gyro_samples: Mutex<Vec<GyroSample>>,
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
        HapticOutcome::Queued(HapticRealizationKind::Waveform)
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
        Ok(OrientationSubscription::new(Arc::new(AtomicU64::new(25)), Box::new(())))
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
}

fn harness(config: ScenarioConfig) -> Harness {
    harness_with_render(config, RenderLoopConfig::default(), ReceiptPolicy::Confirm)
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
    let (sender, events) = shell_event_channel(ShellQueueConfig::default()).expect("очередь");
    let shell = Arc::new(FakeShell::default());
    let output = CollectedOutput::default();
    let policy = Arc::new(ThermalPolicy::new(ThermalPolicyConfig::default()).expect("политика"));
    let (router, starter) = EventRouter::new(RouterParts {
        events,
        services: ShellServices {
            commands: shell.clone(),
            haptics: shell.clone(),
            motion: shell.clone(),
            thermal: shell.clone(),
            diagnostics: shell.clone(),
            vsync: shell.clone(),
            facts: shell.clone(),
        },
        thermal_policy: Arc::clone(&policy),
        scenario_config: config,
        gyro_request: shell_contract::motion::GyroRequest {
            rate: SensorRate::new(NonZeroU32::new(400).expect("частота")),
            retained: Duration::from_secs(5),
        },
        output: Box::new(output.clone()),
        route,
        gpu: GpuServices {
            render: Arc::clone(&render) as Arc<dyn RenderStatusSource>,
            diagnostics: Arc::clone(&diagnostics) as Arc<dyn GpuDiagnostics>,
            render_config,
        },
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

use shell_contract::surface::NativeSurface;

#[test]
fn lifecycle_cycle_reaches_result_with_events_in_order() {
    let mut harness = harness(ScenarioConfig::default());
    harness.shell.counters.lock().expect("замок").live_global_refs = 1;
    harness.start(Scenario::Lifecycle);
    harness.deliver(ShellEvent::Resumed);
    assert!(harness.output.stage_names().is_empty(), "без surface ещё не ready");
    harness.surface_created(1);
    assert!(
        harness.output.stage_names().is_empty(),
        "без размера surface рисовать некуда — ещё не ready"
    );
    harness.surface_changed(1);
    assert_eq!(harness.output.stage_names(), ["ready"]);

    // HOME: пауза, затем Android уничтожает surface — маршрутизатор обязан подтвердить сразу.
    harness.deliver(ShellEvent::Paused);
    assert_eq!(harness.surface_destroyed(1), SurfaceReleaseOutcome::Confirmed);
    assert_eq!(harness.output.stage_names(), ["ready", "backgrounded"]);

    // Возврат: новая surface и передний план. Шумные события в отчёт не идут.
    harness.deliver(ShellEvent::ThermalStatus(ThermalReading {
        status: ThermalStatus::None,
        headroom: ThermalHeadroom::Unavailable,
    }));
    // Порядок возврата как на OnePlus 11: resumed раньше surface_created (сессия 02).
    harness.deliver(ShellEvent::Resumed);
    harness.surface_created(2);
    assert!(
        harness.output.results.lock().expect("замок").is_empty(),
        "без размера — ещё не итог"
    );
    harness.surface_changed(2);

    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "lifecycle");
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        result["report"]["events"],
        serde_json::json!([
            "paused",
            "surface_destroyed",
            "resumed",
            "surface_created",
            "surface_changed"
        ])
    );
    assert_eq!(result["report"]["live_global_refs_at_ready"], 1);
    assert_eq!(result["report"]["live_global_refs_at_end"], 1);
    assert_eq!(result["report"]["activity_recreated"], false);
    assert_eq!(
        harness.router.display().surface_size(),
        Some(SurfaceSize {
            width_px: 1440,
            height_px: 3216
        })
    );
}

#[test]
fn lifecycle_reports_activity_recreation_and_ref_growth() {
    let mut harness = harness(ScenarioConfig::default());
    harness.start(Scenario::Lifecycle);
    harness.deliver(ShellEvent::Resumed);
    harness.surface_created(1);
    harness.surface_changed(1);
    harness.deliver(ShellEvent::Paused);
    harness.surface_destroyed(1);
    {
        let mut counters = harness.shell.counters.lock().expect("замок");
        counters.activity_attachments += 1;
        counters.live_global_refs += 1;
    }
    harness.surface_created(2);
    harness.surface_changed(2);
    harness.deliver(ShellEvent::Resumed);
    let result = harness.output.single_result();
    assert_eq!(result["report"]["activity_recreated"], true);
    assert_eq!(result["report"]["live_global_refs_at_end"], 1);
}

#[test]
fn surface_destroyed_without_scenario_is_confirmed_and_forgotten() {
    let mut harness = harness(ScenarioConfig::default());
    harness.surface_created(7);
    harness.deliver(ShellEvent::Resumed);
    assert!(harness.router.display().is_presenting());
    assert_eq!(harness.surface_destroyed(7), SurfaceReleaseOutcome::Confirmed);
    assert!(!harness.router.display().is_presenting());
}

#[test]
fn stale_surface_destroy_does_not_drop_newer_surface() {
    let mut harness = harness(ScenarioConfig::default());
    harness.surface_created(1);
    harness.surface_created(2);
    harness.deliver(ShellEvent::Resumed);
    // Квитанция старого поколения подтверждается, но текущая surface остаётся.
    assert_eq!(harness.surface_destroyed(1), SurfaceReleaseOutcome::Confirmed);
    assert!(harness.router.display().is_presenting());
}

#[test]
fn permission_scenario_asks_three_times_and_reports_answers() {
    let mut harness = harness(ScenarioConfig::default());
    harness.start(Scenario::Permission);
    harness.router.step(Duration::from_millis(1));
    assert_eq!(harness.shell.permission_requests.load(Ordering::SeqCst), 1);
    harness.deliver(ShellEvent::CameraPermission(CameraPermission::Denied));
    harness.deliver(ShellEvent::CameraPermission(CameraPermission::DeniedPermanently));
    harness.deliver(ShellEvent::CameraPermission(CameraPermission::DeniedPermanently));
    assert_eq!(harness.shell.permission_requests.load(Ordering::SeqCst), 3);
    let stages = harness.output.stages.lock().expect("замок").clone();
    assert_eq!(
        stages,
        vec![
            serde_json::json!({"scenario": "permission", "stage": "dialog_expected", "attempt": 1}),
            serde_json::json!({"scenario": "permission", "stage": "dialog_expected", "attempt": 2}),
            serde_json::json!({"scenario": "permission", "stage": "no_dialog_expected", "attempt": 3}),
        ]
    );
    let result = harness.output.single_result();
    assert_eq!(
        result["report"]["answers"],
        serde_json::json!(["denied", "denied_permanently", "denied_permanently"])
    );
}

#[test]
fn motion_scenario_measures_rate_from_sensor_timestamps() {
    let mut harness = harness(ScenarioConfig {
        motion_duration_ms: 10,
        ..ScenarioConfig::default()
    });
    // Синтетический поток 400 Гц на 1 с: 401 показание через 2,5 мс.
    *harness.shell.gyro_samples.lock().expect("замок") = (0..=400)
        .map(|index| GyroSample {
            time: BoottimeNanos::from_nanos(5_000_000_000 + index * 2_500_000),
            velocity: AngularVelocity {
                x_rad_per_s: 0.0,
                y_rad_per_s: 0.0,
                z_rad_per_s: 0.1,
            },
        })
        .collect();
    harness.start(Scenario::Motion);
    harness.deliver(ShellEvent::DeviceOrientation(DeviceOrientation::Portrait));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed", "{result}");
    let rate = result["report"]["gyro"]["rate_hz"].as_f64().expect("частота");
    assert!((rate - 400.0).abs() < 1e-6, "{rate}");
    assert_eq!(result["report"]["gyro"]["samples"], 401);
    assert_eq!(result["report"]["gyro"]["requested_rate_hz"], 400);
    assert_eq!(result["report"]["orientation"], "portrait");
    assert_eq!(result["report"]["accelerometer_samples"], 25);
}

#[test]
fn platform_scenario_waits_for_surface_then_plays_all_patterns_in_order() {
    let mut harness = harness(ScenarioConfig {
        haptic_interval_ms: 1,
        ..ScenarioConfig::default()
    });
    harness.start(Scenario::Platform);
    harness.router.step(Duration::from_millis(1));
    assert!(
        harness.shell.frame_rates.lock().expect("замок").is_empty(),
        "без surface частоту не просим"
    );
    harness.deliver(ShellEvent::ThermalStatus(ThermalReading {
        status: ThermalStatus::Light,
        headroom: ThermalHeadroom::Measured(0.5),
    }));
    harness.surface_created(1);
    harness.deliver(ShellEvent::Resumed);
    harness.run_until_result();
    assert_eq!(*harness.shell.frame_rates.lock().expect("замок"), vec![60.0]);
    assert_eq!(
        *harness.shell.haptics_played.lock().expect("замок"),
        HapticPattern::ALL.to_vec()
    );
    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(
        report["frame_rate"],
        serde_json::json!({"requested_hz": 60.0, "result": "applied"})
    );
    assert_eq!(report["thermal"]["status"], "light");
    assert_eq!(report["thermal"]["listener"], "registered");
    assert_eq!(report["thermal"]["policy_pressure"], "elevated");
    assert_eq!(report["haptics"]["vibrator"], "present");
    assert_eq!(
        report["haptics"]["patterns"][0],
        serde_json::json!({"pattern": "selection_tick", "outcome": "queued", "realization": "waveform"})
    );
}

#[test]
fn thermal_events_reach_policy_without_scenario() {
    let mut harness = harness(ScenarioConfig::default());
    assert_eq!(harness.policy.pressure(), ThermalPressure::Unknown);
    harness.deliver(ShellEvent::ThermalStatus(ThermalReading {
        status: ThermalStatus::Severe,
        headroom: ThermalHeadroom::Unavailable,
    }));
    assert_eq!(harness.policy.pressure(), ThermalPressure::Critical);
}

#[test]
fn new_scenario_preempts_unfinished_one_with_explicit_failure() {
    let mut harness = harness(ScenarioConfig::default());
    harness.start(Scenario::Lifecycle);
    harness.router.step(Duration::from_millis(1));
    harness.start(Scenario::Permission);
    harness.router.step(Duration::from_millis(1));
    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "lifecycle");
    assert_eq!(result["status"], "failed");
    assert!(result["error"].as_str().expect("error").contains("permission"));
}

#[test]
fn router_stops_when_all_senders_are_gone() {
    let Harness { mut router, sender, .. } = harness(ScenarioConfig::default());
    drop(sender);
    assert_eq!(router.step(Duration::from_millis(10)), RouterStep::Stopped);
}

#[test]
fn router_thread_runs_until_queue_closes() {
    let Harness { router, sender, .. } = harness(ScenarioConfig::default());
    let worker = thread::spawn(move || router.run());
    sender.send(ShellEvent::Resumed);
    drop(sender);
    worker.join().expect("маршрутизатор завершился без паники");
}

/// Сценарий, который паникует на первом событии.
struct PanickingScenario;

impl ObservingScenario for PanickingScenario {
    fn name(&self) -> &'static str {
        "panicking"
    }

    fn start(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        panic!("сценарий сломался");
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }
}

#[test]
fn panicking_scenario_reports_panicked_and_router_keeps_handling_events() {
    crate::panic_report::install_panic_hook();
    let mut harness = harness(ScenarioConfig::default());
    harness.starter.start(Box::new(PanickingScenario)).expect("старт");
    harness.deliver(ShellEvent::Resumed);
    let result = harness.output.single_result();
    assert_eq!(result["scenario"], "panicking");
    assert_eq!(result["status"], "panicked");
    assert_eq!(result["panic"]["message"], "сценарий сломался");
    // Маршрутизатор жив: следующая surface подтверждается, тепло доходит до политики.
    harness.surface_created(3);
    assert_eq!(harness.surface_destroyed(3), SurfaceReleaseOutcome::Confirmed);
    harness.deliver(ShellEvent::ThermalStatus(ThermalReading {
        status: ThermalStatus::Moderate,
        headroom: ThermalHeadroom::Unavailable,
    }));
    assert_eq!(harness.policy.pressure(), ThermalPressure::Throttling);
}
