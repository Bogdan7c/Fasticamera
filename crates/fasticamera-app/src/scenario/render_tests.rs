//! Вердикт `render`: проходной статус и каждое правило отказа по отдельности.

use render_loop::thread::{FaultInfo, FaultKind};

use super::*;
use crate::gpu_test_support::presenting_status;

fn params() -> RenderCheckParams {
    RenderCheckParams {
        expected_period: Duration::from_secs_f64(1.0 / 120.0),
        refresh_tolerance_ratio: 0.05,
        max_miss_ratio: 0.005,
        refresh_settled_after: Duration::from_millis(1234),
        min_coverage_ratio: 0.9,
        synthetic_touch_requested: false,
    }
}

fn verdict_of(status: &RenderStatus, validation: ValidationState) -> Verdict {
    evaluate(status, validation, params())
}

#[test]
fn healthy_status_passes_and_report_carries_all_measurements() {
    let status = presenting_status();
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.violations, Vec::<String>::new());
    let report = &verdict.report;
    assert_eq!(report["frames_presented"], 1200);
    assert_eq!(
        report["pacing"]["histogram"],
        serde_json::json!([0, 1200, 0, 0, 0, 0, 0, 0])
    );
    assert_eq!(report["pacing"]["misses"], 0);
    assert_eq!(report["pacing"]["miss_ratio"], 0.0);
    assert_eq!(report["pacing"]["max_miss_ratio"], 0.005);
    assert_eq!(report["display"]["format"], "B8G8R8A8_UNORM");
    assert_eq!(report["display"]["color_space"], "SRGB_NONLINEAR_KHR");
    assert_eq!(report["display"]["width_px"], 1440);
    assert_eq!(report["display"]["transform"], "Identity");
    assert_eq!(report["display"]["image_count"], 3);
    assert_eq!(report["validation"]["mode"], "enabled");
    assert_eq!(report["validation"]["errors"], 0);
    assert_eq!(report["frame_rate_request"]["result"], "applied");
    let measured = report["refresh"]["measured_period_ms"].as_f64().expect("период");
    assert!((measured - 8.333).abs() < 0.01, "{measured}");
}

#[test]
fn known_deviations_are_reported_and_never_fail_the_verdict() {
    let mut status = presenting_status();
    status.backend.known_deviations.insert("VUID-fake-00001".to_owned(), 12);
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["validation"]["known_deviations"]["VUID-fake-00001"], 12);
}

#[test]
fn pacing_failure_reports_miss_ratio() {
    let mut status = presenting_status();
    status.pacing.verdict = PacingVerdict::Failed {
        miss_ratio: 0.02,
        max_miss_ratio: 0.005,
        misses: 24,
        intervals: 1200,
    };
    status.pacing.misses = 24;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.violations.len(), 1, "{:?}", verdict.violations);
    assert!(verdict.violations[0].contains("24 из 1200"), "{:?}", verdict.violations);
    assert_eq!(
        verdict.report["pacing"]["misses"], 24,
        "отчёт сохраняется и при провале"
    );
}

#[test]
fn not_enough_pacing_data_is_a_violation_not_a_pass() {
    let mut status = presenting_status();
    status.pacing.verdict = PacingVerdict::NotEnoughData;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("данных для вердикта нет"))
    );
}

#[test]
fn unknown_refresh_cycle_fails() {
    let mut status = presenting_status();
    status.display.as_mut().expect("display").refresh_cycle = None;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert!(verdict.violations.iter().any(|text| text.contains("неизвестен")));
    assert!(verdict.report["refresh"]["measured_period_ms"].is_null());
}

#[test]
fn refresh_cycle_off_the_preferred_rate_fails_but_within_tolerance_passes() {
    // 60 Гц вместо 120: платформа не приняла просьбу о частоте.
    let mut status = presenting_status();
    status.display.as_mut().expect("display").refresh_cycle = Some(Duration::from_secs_f64(1.0 / 60.0));
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.violations.len(), 1, "{:?}", verdict.violations);
    assert!(verdict.violations[0].contains("отличается"), "{:?}", verdict.violations);

    // +4 % к периоду — внутри допуска ±5 %.
    let mut near = presenting_status();
    near.display.as_mut().expect("display").refresh_cycle = Some(Duration::from_secs_f64(1.04 / 120.0));
    assert_eq!(
        verdict_of(&near, ValidationState::Enabled).violations,
        Vec::<String>::new()
    );

    // +6 % — уже за допуском.
    let mut far = presenting_status();
    far.display.as_mut().expect("display").refresh_cycle = Some(Duration::from_secs_f64(1.06 / 120.0));
    assert_eq!(verdict_of(&far, ValidationState::Enabled).violations.len(), 1);
}

#[test]
fn no_frames_fails() {
    let mut status = presenting_status();
    status.frames_presented = 0;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("ни одного показанного кадра"))
    );
}

#[test]
fn validation_errors_fail_only_when_layer_is_enabled() {
    let mut status = presenting_status();
    status.backend.validation_errors = 3;
    let enabled = verdict_of(&status, ValidationState::Enabled);
    assert!(enabled.violations.iter().any(|text| text.contains("validation: 3")));
    let disabled = verdict_of(&status, ValidationState::Disabled);
    assert_eq!(
        disabled.violations,
        Vec::<String>::new(),
        "в release слоя нет, счётчик не судим"
    );
    assert_eq!(disabled.report["validation"]["mode"], "disabled");
}

#[test]
fn backend_errors_and_fault_fail() {
    let mut status = presenting_status();
    status.backend_errors = 2;
    status.last_backend_error = Some("queue_present: device lost".to_owned());
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert!(verdict.violations.iter().any(|text| text.contains("device lost")));

    let mut faulty = presenting_status();
    faulty.fault = Some(FaultInfo {
        kind: FaultKind::ClockStopped,
        message: "vsync остановлен".to_owned(),
    });
    let verdict = verdict_of(&faulty, ValidationState::Enabled);
    assert!(verdict.violations.iter().any(|text| text.contains("vsync остановлен")));
}

#[test]
fn presentation_that_stopped_before_the_end_fails() {
    let mut status = presenting_status();
    status.state = RenderState::Paused;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert!(verdict.violations.iter().any(|text| text.contains("показ не идёт")));
}

#[test]
fn report_carries_timing_gaps_and_touch_note() {
    let mut status = presenting_status();
    status.pacing.timing_gaps = 17;
    status.pacing.frames_covered = 1217;
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.report["pacing"]["timing_gaps"], 17);
    assert_eq!(verdict.report["pacing"]["frames_covered"], 1217);
    assert_eq!(verdict.report["refresh"]["settled_after_ms"], 1234.0);
    assert_eq!(verdict.report["pacing"]["rejected_out_of_order"], 0);
    assert_eq!(verdict.report["pacing"]["min_coverage_ratio"], 0.9);
    assert_eq!(verdict.violations, Vec::<String>::new(), "дыры измерения не провал");
}

#[test]
fn sparse_pacing_data_is_a_violation_and_report_shows_the_coverage() {
    let mut status = presenting_status();
    status.pacing.intervals = 100;
    status.pacing.frames_covered = 1000;
    status.pacing.rejected_out_of_order = 3;
    status.pacing.verdict = PacingVerdict::SparseData {
        coverage_ratio: 0.1,
        min_coverage_ratio: 0.9,
        intervals: 100,
        frames_covered: 1000,
    };
    let verdict = verdict_of(&status, ValidationState::Enabled);
    assert_eq!(verdict.violations.len(), 1, "{:?}", verdict.violations);
    assert!(verdict.violations[0].contains("разрежены"), "{:?}", verdict.violations);
    assert!(verdict.violations[0].contains("100") && verdict.violations[0].contains("1000"));
    assert_eq!(verdict.report["pacing"]["coverage_ratio"], 0.1);
    assert_eq!(verdict.report["pacing"]["rejected_out_of_order"], 3);
}

#[test]
fn touch_note_follows_what_the_scenario_actually_requested() {
    let status = presenting_status();
    for requested in [false, true] {
        let verdict = evaluate(
            &status,
            ValidationState::Enabled,
            RenderCheckParams {
                synthetic_touch_requested: requested,
                ..params()
            },
        );
        assert_eq!(verdict.report["refresh"]["synthetic_touch_requested"], requested);
    }
}
