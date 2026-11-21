//! Правила вердикта `standby`: здоровый отчёт проходит, порча одного значения даёт ровно своё нарушение.

use camera_core::lens_status::LensSwitchMethod;

use super::*;
use crate::scenario::camera::standby_test_support::{healthy_report, window};

fn config() -> StandbyScenarioConfig {
    StandbyScenarioConfig::default()
}

fn violations_of(report: &StandbyObservationReport) -> Vec<String> {
    evaluate_standby(&config(), report)
}

fn assert_single(report: &StandbyObservationReport, fragment: &str) {
    let violations = violations_of(report);
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains(fragment),
        "{violations:?} не содержит {fragment:?}"
    );
}

#[test]
fn a_healthy_run_passes() {
    assert_eq!(violations_of(&healthy_report()), Vec::<String>::new());
}

#[test]
fn a_reopened_swap_is_a_violation_even_if_the_picture_switched() {
    let mut report = healthy_report();
    report.record.swaps[1].method = LensSwitchMethod::Reopened;
    assert_single(&report, "reopened");
}

#[test]
fn fewer_than_two_swaps_means_the_run_did_not_finish() {
    let mut report = healthy_report();
    report.record.swaps.pop();
    assert_single(&report, "обменов ролей 1 из 2");
}

#[test]
fn engine_promotions_below_two_are_a_violation() {
    let mut report = healthy_report();
    report.engine.promotions = 1;
    assert_single(&report, "повысил тёплую 1 раз");
}

#[test]
fn any_standby_failure_is_a_violation() {
    let mut report = healthy_report();
    report.engine.standby_failures = 1;
    assert_single(&report, "тёплая отказывала 1 раз");
}

#[test]
fn a_backend_refusal_that_forced_a_reopen_is_a_violation() {
    let mut report = healthy_report();
    report.engine.promotion_fallbacks = 2;
    assert_single(&report, "бэкенд отверг обмен ролей 2 раз");
}

#[test]
fn a_leaked_lease_is_a_violation() {
    let mut report = healthy_report();
    report.stream.leaked_leases = 1;
    assert_single(&report, "утекло лизингов кадров: 1");
}

#[test]
fn a_standby_that_returned_no_frames_did_not_stream() {
    let mut report = healthy_report();
    report.stream.standby_frames_returned = 0;
    assert_single(&report, "не стримила");
}

#[test]
fn the_frame_stream_must_have_seen_both_promotions() {
    let mut report = healthy_report();
    report.stream.standby_promotions = 1;
    assert_single(&report, "поток кадров провёл обменов 1");
}

#[test]
fn backend_without_offered_standby_results_is_a_violation() {
    let mut report = healthy_report();
    report.backend[0].results_offered = 0;
    assert_single(&report, "не предложил потоку ни одного результата");
}

#[test]
fn low_fps_of_either_camera_while_both_run_is_a_violation() {
    let mut primary = healthy_report();
    primary.record.windows[1] = window("both_running", 54.9, 59.5, true);
    assert_single(&primary, "показываемая камера 54.9");

    let mut standby = healthy_report();
    standby.record.windows[2] = window("after_to_neighbour", 59.7, 40.0, true);
    assert_single(&standby, "тёплая камера 40.0");
}

#[test]
fn low_fps_of_the_lone_camera_is_not_judged_by_the_both_cameras_rule() {
    let mut report = healthy_report();
    report.record.windows[0] = window("alone", 30.0, 0.0, false);
    assert_eq!(violations_of(&report), Vec::<String>::new());
}

#[test]
fn a_run_without_any_both_cameras_window_proves_nothing() {
    let mut report = healthy_report();
    report.record.windows.retain(|window| !window.both_cameras_running);
    assert_single(&report, "не замерена частота");
}

#[test]
fn standbys_left_open_after_release_are_a_violation() {
    let mut report = healthy_report();
    report.record.release_ms = None;
    assert_single(&report, "одна открытая камера не достигнута");
}

#[test]
fn ending_on_a_foreign_lens_is_a_violation() {
    let mut report = healthy_report();
    report.final_role = Some("ultra_wide");
    assert_single(&report, "в конце активна роль");
}

#[test]
fn a_run_failure_is_reported_and_does_not_hide_other_problems() {
    let mut report = healthy_report();
    report.run_failure = Some("home: не дождались".to_owned());
    report.stream.leaked_leases = 3;
    let violations = violations_of(&report);
    assert!(
        violations.iter().any(|text| text.contains("не дождались")),
        "{violations:?}"
    );
    assert!(
        violations.iter().any(|text| text.contains("утекло лизингов")),
        "{violations:?}"
    );
}
