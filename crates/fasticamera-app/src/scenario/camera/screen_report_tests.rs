#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

use super::super::engine_watch::RunFailure;
use super::super::viewfinder_metrics::GpuTimeMs;
use super::super::viewfinder_smoke_report::tests::healthy;
use super::*;
use crate::screen_scenario_config::ScreenScenarioConfig;

fn time(p95_ms: f64) -> GpuTimeMs {
    GpuTimeMs {
        samples: 600,
        p50_ms: p95_ms / 2.0,
        p95_ms,
        max_ms: p95_ms + 0.4,
    }
}

fn step(name: &'static str) -> StepRecord {
    StepRecord {
        name,
        ok: true,
        inferred: false,
        observed: "тапы 1".to_owned(),
    }
}

/// Здоровый прогон: все шаги выполнены, экран без отказов, слой UI в бюджете.
fn healthy_observation() -> ScreenObservation {
    ScreenObservation {
        viewfinder: healthy(),
        steps: vec![step("focus_tap_center"), step("volume_up_shutter")],
        unfinished: Vec::new(),
        plan_failure: None,
        screen: ScreenCounters {
            frames_produced: 1_200,
            touches_received: 300,
            ..ScreenCounters::default()
        },
        overlay: OverlayWindow {
            active_source: Some(EXPECTED_SOURCE.to_owned()),
            installs: 0,
            removals: 0,
            frames_drawn: 1_200,
            frames_failed: 0,
            source_failures: 0,
            limit_overflows: 0,
            // Экран PHOTO без стекла (14p): пирамида в окне не строится, и это не провал.
            pyramid_rebuilds: 0,
            pyramid_reuses: 0,
            gpu_time: Some(time(1.5)),
            last_error: None,
        },
        warmup: WarmupReport {
            frames: 30,
            duration_ms: 520,
        },
        budget: UiGpuBudget {
            refresh_hz: 60.0,
            fraction: 0.5,
            budget_ms: round2(ScreenScenarioConfig::default().ui_gpu_budget_ms(60.0)),
        },
    }
}

fn verdict(observation: &ScreenObservation) -> Vec<String> {
    evaluate_screen(&ViewfinderSmokeConfig::default(), observation)
}

fn only_violation(observation: &ScreenObservation) -> String {
    let violations = verdict(observation);
    assert_eq!(violations.len(), 1, "{violations:?}");
    violations.into_iter().next().unwrap()
}

#[test]
fn a_healthy_run_passes() {
    assert_eq!(verdict(&healthy_observation()), Vec::<String>::new());
}

#[test]
fn a_photo_screen_without_glass_passes_without_a_blur_pyramid_and_the_glass_showcase_still_needs_it() {
    // Решение 297: здоровый прогон экрана — с нулём перестроек пирамиды (стекла в PHOTO нет).
    let observation = healthy_observation();
    assert_eq!(observation.overlay.pyramid_rebuilds, 0);
    assert!(verdict(&observation).is_empty(), "{:?}", verdict(&observation));
    // А витрине стекла пирамида по-прежнему обязательна: та же картина счётчиков для неё — провал.
    let glass = super::super::glass_report::overlay_violations(
        &observation.overlay,
        super::super::glass_report::BackdropBlurExpectation::MustBeBuilt,
    );
    assert!(
        glass.iter().any(|violation| violation.contains("pyramid_rebuilds = 0")),
        "{glass:?}"
    );
}

#[test]
fn a_failed_step_fails_the_run_with_the_step_reason_verbatim() {
    let mut observation = healthy_observation();
    observation.plan_failure = Some("шаг focus_tap_center: триггеры до HAL: ожидали ровно 1, увидели 0".to_owned());
    let violation = only_violation(&observation);
    assert!(violation.contains("триггеры до HAL"), "{violation}");
}

#[test]
fn unfinished_steps_are_named_when_there_is_no_other_reason() {
    let mut observation = healthy_observation();
    observation.unfinished = vec!["ev_icon_tap", "volume_up_shutter"];
    let violation = only_violation(&observation);
    assert!(
        violation.contains("не выполнены шаги плана") && violation.contains("ev_icon_tap, volume_up_shutter"),
        "{violation}"
    );
}

#[test]
fn each_screen_counter_fails_with_its_own_number() {
    /// Как испортить счётчик и что должно быть в тексте нарушения.
    type Case = (fn(&mut ScreenCounters), &'static str);
    let cases: [Case; 9] = [
        (|screen| screen.frame_failures = 2, "отказов кадра экрана: 2"),
        (
            |screen| screen.effects_overflowed = 1,
            "переполнений очереди эффектов экрана: 1",
        ),
        (
            |screen| screen.unhandled_gestures = 1,
            "событий жестов без владельца: 1",
        ),
        (
            |screen| screen.orphan_pan_continuations = 2,
            "продолжений пана раздела без начала: 2",
        ),
        (
            |screen| screen.finger_focus_refusals = 3,
            "отказов «картинки под пальцем» без ведомых узлов: 3",
        ),
        (
            |screen| screen.input_overflowed = 4,
            "потерь ввода из-за переполнения канала: 4",
        ),
        (
            |screen| screen.input_dropped_without_screen = 3,
            "вводов, отброшенных без экрана: 3",
        ),
        (|screen| screen.executor_refusals = 1, "отказов исполнителя эффектов: 1"),
        (
            |screen| screen.results_with_unknown_origin = 2,
            "с неопознанным происхождением",
        ),
    ];
    for (break_it, expected) in cases {
        let mut observation = healthy_observation();
        break_it(&mut observation.screen);
        let violation = only_violation(&observation);
        assert!(violation.contains(expected), "{violation} не содержит {expected}");
    }
}

#[test]
fn a_frame_failure_carries_the_last_failure_text() {
    let mut observation = healthy_observation();
    observation.screen.frame_failures = 1;
    observation.screen.last_frame_failure = Some("сцена не собрана".to_owned());
    assert!(only_violation(&observation).contains("сцена не собрана"));
}

#[test]
fn a_screen_that_drew_nothing_fails() {
    let mut observation = healthy_observation();
    observation.screen.frames_produced = 0;
    assert!(only_violation(&observation).contains("frames_produced = 0"));
}

#[test]
fn another_ui_source_in_the_render_fails() {
    let mut observation = healthy_observation();
    observation.overlay.active_source = Some("glass_showcase".to_owned());
    let violation = only_violation(&observation);
    assert!(
        violation.contains("glass_showcase") && violation.contains("camera_screen"),
        "{violation}"
    );
}

#[test]
fn ui_gpu_time_over_half_the_frame_period_fails_and_equal_passes() {
    let mut observation = healthy_observation();
    assert_eq!(observation.budget.budget_ms, 8.33, "60 Гц × 0,5, решение 90");
    observation.overlay.gpu_time = Some(time(8.33));
    assert_eq!(verdict(&observation), Vec::<String>::new());
    observation.overlay.gpu_time = Some(time(8.34));
    assert!(only_violation(&observation).contains("GPU-время кадра UI"));
    observation.overlay.gpu_time = None;
    assert!(only_violation(&observation).contains("нет замеров"));
}

#[test]
fn late_frames_are_a_number_and_not_a_failure() {
    let mut observation = healthy_observation();
    observation.viewfinder.window.present_target.vsync_shift.late_1 = 400;
    assert_eq!(verdict(&observation), Vec::<String>::new(), "решение 91");
    assert_eq!(observation.late_frames(), 400);
    assert_eq!(observation.summary()["late_frames"], 400);
}

#[test]
fn a_viewfinder_failure_is_the_only_violation() {
    let mut observation = healthy_observation();
    observation.viewfinder.failure = Some(RunFailure::NeverStreaming { last_state: "Open" });
    observation.plan_failure = Some("шаг: ...".to_owned());
    assert_eq!(verdict(&observation).len(), 1);
}

#[test]
fn the_summary_names_every_step_and_fits_the_logcat_line() {
    let mut observation = healthy_observation();
    observation.steps = (0..18).map(|_| step("menu_probe_tap_closed")).collect();
    let summary = observation.summary();
    assert_eq!(summary["steps"].as_array().map(Vec::len), Some(18));
    assert_eq!(
        summary["steps"][0],
        serde_json::json!({ "name": "menu_probe_tap_closed", "ok": true })
    );
    assert!(summary.to_string().len() < 3_000, "{} байт", summary.to_string().len());
}

#[test]
fn the_summary_says_that_ev_is_proven_only_up_to_the_engine() {
    let summary = healthy_observation().summary();
    assert_eq!(summary["ev_proof"], EV_PROOF_ENGINE_DESIRED);
    assert_eq!(
        summary["ev_proof"], "engine_desired_value",
        "контракт с scripts/smoke_checks_screen.py"
    );
}

/// Решение 105: под плотными касаниями переполнения джиттер-буфера — число в отчёте, а не провал.
/// Те же 9 переполнений за окно в `viewfinder`/`glass` провалили бы прогон (порог — дрейф часов, 4 в минуту).
#[test]
fn jitter_overflow_under_touches_is_reported_but_does_not_fail_the_screen_run() {
    let mut observation = healthy_observation();
    observation.viewfinder.pacing.jitter_overflow = 9;

    assert_eq!(verdict(&observation), Vec::<String>::new());
    assert_eq!(
        observation.summary()["viewfinder"]["jitter_overflow"],
        9,
        "число не спрятано"
    );
    let same_without_touches = super::super::viewfinder_smoke_report::evaluate_viewfinder(
        &ViewfinderSmokeConfig::default(),
        &observation.viewfinder,
    );
    assert!(
        same_without_touches
            .iter()
            .any(|violation| violation.contains("джиттер-буфер переполнился 9 раз")),
        "без касаний порог действует: {same_without_touches:?}"
    );
}

/// Ослаблен только порог переполнений: остальные проверки видоискателя под касаниями по-прежнему валят прогон.
#[test]
fn other_viewfinder_checks_still_fail_the_screen_run() {
    let mut observation = healthy_observation();
    observation.viewfinder.window.isp.cpu_copies = 1;

    let violation = only_violation(&observation);
    assert!(violation.contains("видоискатель: CPU-копий кадров: 1"), "{violation}");
}

#[test]
fn a_pipeline_created_in_a_frame_during_the_screen_run_fails_the_run() {
    let mut observation = healthy_observation();
    observation.viewfinder.window.pipeline_variants.created_in_frame = 2;
    assert!(only_violation(&observation).contains("создано пайплайнов в кадре: 2"));
}
