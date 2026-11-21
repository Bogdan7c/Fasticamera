//! Вердикт бесшовных шагов: каждое правило ловит свою порчу одного значения и молчит на здоровом шаге.

use camera_core::lens_status::LensSwitchMethod;

use super::*;
use crate::scenario::camera::zoom_record::SwitchTiming;
use crate::scenario::camera::zoom_seamless_record::{FpsPair, SwitchGap};
use crate::scenario::camera::zoom_test_support::{healthy_release_step, healthy_report, healthy_seamless_switch_step};
use crate::zoom_scenario_config::ZoomScenarioConfig;

fn plan() -> SeamlessPlan {
    SeamlessPlan::default()
}

fn healthy() -> ZoomStepRecord {
    healthy_seamless_switch_step("11_wheel_seamless_out", "out")
}

#[test]
fn a_healthy_seamless_switch_and_release_have_no_violations() {
    assert_eq!(seamless_step_violations(&plan(), &healthy()), Vec::<String>::new());
    assert_eq!(
        seamless_step_violations(&plan(), &healthy_release_step("13_wheel_seamless_release")),
        Vec::<String>::new()
    );
}

#[test]
fn an_ordinary_step_is_not_judged_here() {
    let ordinary = crate::scenario::camera::zoom_test_support::healthy_switch_step("01_tap_0.6");
    assert!(seamless_step_violations(&plan(), &ordinary).is_empty());
}

#[test]
fn a_reopened_switch_fails_and_a_promoted_one_passes() {
    let mut step = healthy();
    step.switch = step.switch.map(|switch| SwitchTiming {
        method: LensSwitchMethod::Reopened,
        ..switch
    });
    let violations = seamless_step_violations(&plan(), &step);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(violations[0].contains("Reopened"), "{violations:?}");
}

#[test]
fn held_black_hidden_and_extra_switch_events_each_fail_separately() {
    let mut held = healthy();
    held.held_frames = 3;
    let mut black = healthy();
    black.continuity.black_frames_before_first_picture = 1;
    let mut hidden = healthy();
    hidden.continuity.frames_hidden_by_continuity = 2;
    let mut twice = healthy();
    twice.seamless.as_mut().expect("запись").switch_events = 2;
    for (step, needle) in [
        (held, "удержанных кадров 3"),
        (black, "чёрных кадров 1"),
        (hidden, "скрытых непрерывностью 2"),
        (twice, "событий LensSwitched 2"),
    ] {
        let violations = seamless_step_violations(&plan(), &step);
        assert_eq!(violations.len(), 1, "{needle}: {violations:?}");
        assert!(violations[0].contains(needle), "{needle}: {violations:?}");
    }
}

#[test]
fn a_gap_over_the_threshold_slow_fps_and_missing_measurements_fail() {
    let mut gap = healthy();
    gap.seamless.as_mut().expect("запись").gap = Some(SwitchGap {
        gap_ms: 60.0,
        period_ms: 16.7,
    });
    assert!(seamless_step_violations(&plan(), &gap)[0].contains("разрыв показа"));

    let mut slow = healthy();
    slow.seamless.as_mut().expect("запись").fps = Some(FpsPair {
        primary: 59.9,
        standby: 41.0,
    });
    assert!(seamless_step_violations(&plan(), &slow)[0].contains("41.0"));

    let mut unmeasured = healthy();
    {
        let record = unmeasured.seamless.as_mut().expect("запись");
        record.gap = None;
        record.fps = None;
        record.ready_ms = None;
    }
    assert_eq!(seamless_step_violations(&plan(), &unmeasured).len(), 3);
    // Кольцо потеряло записи: разрыв «не измерен», а не «плохой».
    unmeasured.ring_overran = true;
    assert_eq!(seamless_step_violations(&plan(), &unmeasured).len(), 2);
}

#[test]
fn a_release_that_never_closed_fails() {
    let mut step = healthy_release_step("13_wheel_seamless_release");
    step.seamless.as_mut().expect("запись").closed_ms = None;
    assert!(seamless_step_violations(&plan(), &step)[0].contains("тёплая закрыта"));
}

#[test]
fn totals_fail_on_leaks_standby_failures_fallbacks_and_missing_promotions() {
    let mut report = healthy_report();
    report.steps = vec![
        healthy_seamless_switch_step("11_wheel_seamless_out", "out"),
        healthy_seamless_switch_step("12_wheel_seamless_back", "back"),
        healthy_release_step("13_wheel_seamless_release"),
    ];
    report.totals.standby_promotions = 2;
    assert_eq!(seamless_totals_violations(&report), Vec::<String>::new());

    report.totals.leaked_leases = 1;
    report.totals.standby_failures = 1;
    report.totals.promotion_fallbacks = 1;
    report.totals.standby_promotions = 1;
    assert_eq!(seamless_totals_violations(&report).len(), 4);
}

#[test]
fn finger_driven_steps_are_not_judged_by_the_tap_time_thresholds() {
    use crate::scenario::camera::zoom_verdict::evaluate_zoom;
    let config = ZoomScenarioConfig::default();
    let mut report = healthy_report();
    let mut slow = healthy();
    slow.switch = slow.switch.map(|switch| SwitchTiming {
        observed_ms: config.max_switch_ms * 3,
        ..switch
    });
    slow.first_live_frame_ms = Some(config.max_first_live_frame_ms as f64 * 3.0);
    report.steps = vec![slow];
    report.totals.standby_promotions = 1;
    assert_eq!(evaluate_zoom(&config, &report), Vec::<String>::new());
}
