//! Отчёт `zoom`: сводка компактна и несёт нужные числа, подробности — по записи на шаг, худший случай влезает в строку.

use render_loop::viewfinder::ContinuityCounters;

use super::*;
use crate::scenario::camera::result_budget::judge_within_budget;
use crate::scenario::camera::result_budget::worst_case::assert_failure_line_fits;
use crate::scenario::camera::zoom_test_support::{healthy_report, healthy_switch_step};

#[test]
fn summary_carries_the_numbers_the_phone_check_reads() {
    let summary = healthy_report().summary();
    assert_eq!(summary["steps_planned"], 3);
    assert_eq!(summary["steps_done"], 3);
    assert_eq!(summary["switches_observed"], 1);
    assert_eq!(summary["max_switch_ms"], 400);
    assert_eq!(summary["max_first_live_frame_ms"], 520.0);
    assert_eq!(summary["black_frames_while_picture_expected"], 0);
    assert_eq!(summary["final"]["ratio_milli"], 1_000);
    assert_eq!(summary["final"]["facing"], "back");
    assert_eq!(summary["final"]["role"], "wide");
}

#[test]
fn details_have_totals_first_and_one_record_per_step() {
    let report = healthy_report();
    let items = report.detail_items();
    assert_eq!(items.len(), 1 + report.steps.len());
    assert!(items[0].get("totals").is_some());
    assert_eq!(items[1]["name"], "01_tap_0.6");
    assert_eq!(items[3]["haptics"]["count"], 15);
}

#[test]
fn totals_take_continuity_deltas_not_absolute_counters() {
    let start = ContinuityCounters {
        held_frames_presented: 10,
        black_frames_while_picture_expected: 0,
        holds_revealed_by_timeout: 1,
        black_frames_picture_unavailable: 4,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        held_frames_presented: 70,
        black_frames_while_picture_expected: 2,
        holds_revealed_by_timeout: 1,
        black_frames_picture_unavailable: 6,
        ..ContinuityCounters::default()
    };
    let totals = ZoomTotals::default().with_continuity(&start, &end);
    assert_eq!(
        (totals.held_frames, totals.black_frames_while_picture_expected),
        (60, 2)
    );
    assert_eq!(
        totals.reveal_faults,
        RevealFaults {
            black_picture_unavailable: 2,
            ..RevealFaults::default()
        },
        "прирост считается от начала окна: старый таймаут (1 → 1) не в счёт"
    );
}

#[test]
fn max_times_ignore_steps_without_a_switch() {
    let mut report = healthy_report();
    report.steps.push(healthy_switch_step("04_x"));
    report.steps[3].switch = None;
    report.steps[3].first_live_frame_ms = None;
    assert_eq!(report.max_switch_ms(), Some(400));
    assert_eq!(report.max_first_live_frame_ms(), Some(520.0));
}

#[test]
fn the_worst_failing_summary_fits_the_result_line() {
    let mut report = healthy_report();
    // Худший случай: длинные кириллические причины в сводке и в нарушениях.
    report.run_failure = Some(crate::scenario::camera::result_budget::worst_case::long_reason("zoom"));
    let violations: Vec<String> = (0..12)
        .map(|index| crate::scenario::camera::result_budget::worst_case::long_reason(&format!("шаг {index}")))
        .collect();
    let verdict = judge_within_budget(report.summary(), violations);
    assert_failure_line_fits("zoom", verdict.outcome);
}

#[test]
fn black_frames_before_first_picture_grow_into_totals_and_the_summary() {
    let start = ContinuityCounters {
        black_frames_before_first_picture: 5,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        black_frames_before_first_picture: 9,
        ..ContinuityCounters::default()
    };
    let mut report = healthy_report();
    report.totals = ZoomTotals::default().with_continuity(&start, &end);
    assert_eq!(
        report.totals.black_frames_before_first_picture, 4,
        "холодный старт до окна не в счёт"
    );
    assert_eq!(report.summary()["black_frames_before_first_picture"], 4);
}
