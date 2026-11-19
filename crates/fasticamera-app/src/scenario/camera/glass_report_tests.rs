#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

use std::time::Duration;

use render_loop::thread::DurationSummary;

use super::super::engine_watch::RunFailure;
use super::super::viewfinder_smoke_report::tests::healthy;
use super::*;

fn time(p95_ms: f64) -> GpuTimeMs {
    GpuTimeMs {
        samples: 600,
        p50_ms: p95_ms / 2.0,
        p95_ms,
        max_ms: p95_ms + 0.4,
    }
}

/// Здоровое стекло: источник стоит, 1800 кадров нарисовано, пирамида строилась и переиспользовалась,
/// p95 GPU UI 1,5 мс при бюджете 4,17 мс (60 Гц × 0,25).
fn healthy_observation() -> GlassObservation {
    let config = GlassScenarioConfig::default();
    GlassObservation {
        viewfinder: healthy(),
        overlay: OverlayWindow {
            active_source: Some("glass_showcase".to_owned()),
            installs: 1,
            removals: 0,
            frames_drawn: 1_800,
            frames_failed: 0,
            source_failures: 0,
            limit_overflows: 0,
            pyramid_rebuilds: 12,
            pyramid_reuses: 1_788,
            gpu_time: Some(time(1.5)),
            last_error: None,
        },
        warmup: WarmupReport {
            frames: 30,
            duration_ms: 520,
        },
        budget: UiGpuBudget::of(&config, 60.0),
        gravity: GravityReport::Active {
            source: "dedicated_sensor",
            samples_in_window: 6_000,
        },
        removal: SourceRemoval::Delivered,
    }
}

fn verdict(observation: &GlassObservation) -> Vec<String> {
    evaluate_glass(
        &GlassScenarioConfig::default(),
        &ViewfinderSmokeConfig::default(),
        observation,
    )
}

fn only_violation(observation: &GlassObservation) -> String {
    let violations = verdict(observation);
    assert_eq!(violations.len(), 1, "{violations:?}");
    violations.into_iter().next().unwrap()
}

#[test]
fn healthy_glass_passes_with_no_notes() {
    let observation = healthy_observation();
    assert_eq!(verdict(&observation), Vec::<String>::new());
    assert_eq!(glass_notes(&observation), Vec::<String>::new());
}

#[test]
fn a_missing_source_fails_with_the_source_reason_only() {
    let mut observation = healthy_observation();
    observation.overlay.active_source = None;
    let violation = only_violation(&observation);
    assert!(violation.contains("не установлен"), "{violation}");
}

#[test]
fn zero_drawn_frames_fail_even_with_an_installed_source() {
    let mut observation = healthy_observation();
    observation.overlay.frames_drawn = 0;
    let violation = only_violation(&observation);
    assert!(violation.contains("frames_drawn = 0"), "{violation}");
}

#[test]
fn each_failure_counter_fails_and_carries_the_last_error_text() {
    for pick in [
        (|overlay: &mut OverlayWindow| overlay.frames_failed = 3) as fn(&mut OverlayWindow),
        |overlay| overlay.source_failures = 3,
        |overlay| overlay.limit_overflows = 3,
    ] {
        let mut observation = healthy_observation();
        pick(&mut observation.overlay);
        observation.overlay.last_error = Some("список рисования отклонён".to_owned());
        let violation = only_violation(&observation);
        assert!(violation.contains("= 3, ожидалось 0"), "{violation}");
        assert!(violation.contains("список рисования отклонён"), "{violation}");
    }
}

#[test]
fn a_blur_pyramid_that_was_never_built_fails() {
    let mut observation = healthy_observation();
    observation.overlay.pyramid_rebuilds = 0;
    let violation = only_violation(&observation);
    assert!(violation.contains("pyramid_rebuilds = 0"), "{violation}");
}

#[test]
fn gpu_time_p95_over_the_budget_fails_and_equal_to_the_budget_passes() {
    let mut observation = healthy_observation();
    // Бюджет при 60 Гц: 16,67 × 0,25 = 4,17 мс (округлён до сотых).
    assert_eq!(observation.budget.budget_ms, 4.17);
    observation.overlay.gpu_time = Some(time(4.17));
    assert_eq!(verdict(&observation), Vec::<String>::new(), "ровно в бюджет — норма");
    observation.overlay.gpu_time = Some(time(4.18));
    let violation = only_violation(&observation);
    assert!(violation.contains("4.18") && violation.contains("4.17"), "{violation}");
}

#[test]
fn the_budget_follows_the_configured_refresh_rate() {
    let mut observation = healthy_observation();
    observation.budget = UiGpuBudget::of(&GlassScenarioConfig::default(), 120.0);
    assert_eq!(observation.budget.budget_ms, 2.08);
    observation.overlay.gpu_time = Some(time(3.0));
    only_violation(&observation);
}

#[test]
fn missing_gpu_time_samples_fail() {
    let mut observation = healthy_observation();
    observation.overlay.gpu_time = None;
    let violation = only_violation(&observation);
    assert!(violation.contains("нет замеров GPU-времени"), "{violation}");
}

#[test]
fn frames_shown_after_their_vsync_fail_and_the_allowance_is_a_parameter() {
    let mut observation = healthy_observation();
    observation.viewfinder.window.present_target.vsync_shift.late_1 = 2;
    observation.viewfinder.window.present_target.vsync_shift.late_3_plus = 1;
    assert_eq!(observation.missed_frames(), 3);
    let violation = only_violation(&observation);
    assert!(violation.contains("позже задуманного vsync: 3"), "{violation}");
    let tolerant = GlassScenarioConfig {
        max_missed_frames: 3,
        ..GlassScenarioConfig::default()
    };
    assert_eq!(
        evaluate_glass(&tolerant, &ViewfinderSmokeConfig::default(), &observation),
        Vec::<String>::new()
    );
}

#[test]
fn an_undelivered_removal_fails_but_not_installed_does_not() {
    let mut observation = healthy_observation();
    observation.removal = SourceRemoval::NotDelivered {
        reason: "поток рендера уже завершён".to_owned(),
    };
    let violation = only_violation(&observation);
    assert!(
        violation.contains("не снят") && violation.contains("уже завершён"),
        "{violation}"
    );
    observation.removal = SourceRemoval::NotInstalled;
    assert_eq!(verdict(&observation), Vec::<String>::new());
}

#[test]
fn viewfinder_rules_apply_with_a_prefix_validation_errors_included() {
    let mut observation = healthy_observation();
    if let Some(validation) = observation.viewfinder.validation.as_mut() {
        validation.errors = 2;
    }
    let violation = only_violation(&observation);
    assert!(violation.starts_with("видоискатель: "), "{violation}");
    assert!(violation.contains("validation"), "{violation}");

    let mut slow_sensor = healthy_observation();
    slow_sensor.viewfinder.sensor_fps = Some(30.0);
    assert!(
        verdict(&slow_sensor)
            .iter()
            .any(|violation| violation.starts_with("видоискатель: ") && violation.contains("частота сенсора")),
    );
}

#[test]
fn a_camera_failure_is_the_only_reported_reason() {
    let mut observation = healthy_observation();
    observation.viewfinder.failure = Some(RunFailure::NeverStreaming { last_state: "opening" });
    observation.overlay = OverlayWindow::default();
    let violations = verdict(&observation);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("opening"), "{violations:?}");
}

#[test]
fn gravity_trouble_is_a_note_not_a_failure() {
    let mut observation = healthy_observation();
    observation.gravity = GravityReport::Unavailable {
        reason: "на телефоне нет датчика гравитации".to_owned(),
    };
    assert_eq!(verdict(&observation), Vec::<String>::new());
    let notes = glass_notes(&observation);
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("нет датчика гравитации"), "{notes:?}");

    observation.gravity = GravityReport::Active {
        source: "filtered_accelerometer",
        samples_in_window: 0,
    };
    assert_eq!(verdict(&observation), Vec::<String>::new());
    let notes = glass_notes(&observation);
    assert!(
        notes[0].contains("filtered_accelerometer") && notes[0].contains("ни одного показания"),
        "{notes:?}"
    );
}

#[test]
fn overlay_window_is_the_difference_of_counters_and_keeps_end_state() {
    let before = UiOverlayDiagnostics {
        installs: 0,
        frames_drawn: 5,
        pyramid_rebuilds: 1,
        ..UiOverlayDiagnostics::default()
    };
    let after = UiOverlayDiagnostics {
        active_source: Some("glass_showcase".to_owned()),
        installs: 1,
        frames_drawn: 1_805,
        pyramid_rebuilds: 4,
        pyramid_reuses: 1_800,
        gpu_time: Some(DurationSummary {
            samples: 600,
            p50: Duration::from_micros(900),
            p95: Duration::from_micros(1_500),
            max: Duration::from_micros(2_000),
        }),
        last_error: Some("x".to_owned()),
        ..UiOverlayDiagnostics::default()
    };
    let window = OverlayWindow::between(&before, &after);
    assert_eq!(window.frames_drawn, 1_800);
    assert_eq!(window.pyramid_rebuilds, 3);
    assert_eq!(window.pyramid_reuses, 1_800);
    assert_eq!(window.installs, 1);
    assert_eq!(window.active_source.as_deref(), Some("glass_showcase"));
    assert_eq!(window.gpu_time.map(|time| time.p95_ms), Some(1.5));
    assert_eq!(window.last_error.as_deref(), Some("x"));
}

#[test]
fn the_summary_carries_the_contract_fields_and_fits_the_result_line() {
    let observation = healthy_observation();
    let summary = observation.summary(&GlassScenarioConfig::default());
    assert_eq!(summary["overlay"]["frames_drawn"], 1_800);
    assert_eq!(summary["overlay"]["gpu_time"]["p95_ms"], 1.5);
    assert_eq!(summary["ui_gpu_budget"]["budget_ms"], 4.17);
    assert_eq!(summary["warmup"]["frames"], 30);
    assert_eq!(summary["warmup"]["duration_ms"], 520);
    assert_eq!(summary["missed_frames"], 0);
    assert_eq!(summary["missed_frames_allowed"], 0);
    assert_eq!(summary["source_removal"]["state"], "delivered");
    assert_eq!(summary["gravity"]["state"], "active");
    assert_eq!(summary["gravity"]["samples_in_window"], 6_000);
    assert_eq!(summary["viewfinder"]["cpu_copies"], 0);
    assert!(summary["validation"]["errors"].is_number());
    let bytes = summary.to_string().len();
    assert!(
        bytes < 2_500,
        "сводка {bytes} байт: до бюджета строки результата должен оставаться запас"
    );
}

#[test]
fn details_hold_the_overlay_and_the_full_viewfinder_summary() {
    let details = healthy_observation().detail_items();
    assert_eq!(details[0]["overlay"]["frames_drawn"], 1_800);
    assert_eq!(details[0]["warmup"]["frames"], 30);
    assert!(details[1]["viewfinder_summary"]["pacing"].is_object());
    assert!(details.len() > 2, "части наблюдения видоискателя идут следом");
}
