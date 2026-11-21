//! Функциональные тесты маршрутизатора на простых сценариях (жизненный цикл, разрешение, датчики движения, платформа),
//! вытеснение сценариев, паника сценария и подсказки оператору. Fake-оболочка и стенд (`Harness`) — из
//! `event_router_tests`.

use super::*;

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
        operator_prompt_hold_ms: 1,
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
    // Подсказка владельцу: показана ПОСЛЕ паттернов, снята до итога, исходы в отчёте.
    let shown = harness.shell.prompts.shown();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].headline(), "ПРОВЕРКА ПОДСКАЗКИ");
    assert_eq!(
        report["operator_prompt"],
        serde_json::json!({"show": "queued", "clear": "queued", "hold_ms": 1})
    );
    assert!(
        harness
            .output
            .stage_names()
            .contains(&"operator_prompt_shown".to_owned())
    );
    assert_eq!(
        harness.shell.prompts.events().first(),
        Some(&crate::operator_prompt_fake::PromptEvent::Shown(shown[0].clone())),
        "первым делом показ, снятие после"
    );
    assert!(harness.shell.prompts.clear_count() >= 1);
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

#[test]
fn platform_scenario_reports_a_prompt_the_phone_could_not_show() {
    let mut harness = harness(ScenarioConfig {
        haptic_interval_ms: 1,
        operator_prompt_hold_ms: 1,
        ..ScenarioConfig::default()
    });
    harness.shell.prompts.set_outcome(Some(CommandOutcome::NotAttached));
    harness.start(Scenario::Platform);
    harness.surface_created(1);
    harness.deliver(ShellEvent::Resumed);
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        result["report"]["operator_prompt"],
        serde_json::json!({"show": "not_attached", "clear": "not_attached", "hold_ms": 1})
    );
}

#[test]
fn any_scenario_end_clears_the_operator_prompt_and_a_preempted_one_too() {
    let mut harness = harness(ScenarioConfig::default());
    harness.start(Scenario::Lifecycle);
    harness.router.step(Duration::from_millis(1));
    assert_eq!(
        harness.shell.prompts.clear_count(),
        0,
        "пока сценарий идёт, подсказку не трогают"
    );
    harness.start(Scenario::Permission);
    harness.router.step(Duration::from_millis(1));
    assert_eq!(
        harness.shell.prompts.clear_count(),
        1,
        "вытесненный сценарий закончился провалом, и подсказка снята до строки результата"
    );
}
