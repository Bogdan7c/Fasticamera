use std::time::Duration;

use camera_contract::capture_result::FrameNumber;
use camera_contract::three_a::{BurstId, PointRequestId};

use super::*;

fn result(frame: i64, origin: RequestOrigin) -> LastThreeAResult {
    LastThreeAResult {
        frame_number: FrameNumber(frame),
        sensor_timestamp: None,
        origin,
        af_state: None,
        ae_state: None,
        focus_distance_diopters: None,
        scene_flicker: None,
        exposure_time: Some(Duration::from_millis(2)),
        sensitivity_iso: Some(400),
    }
}

#[test]
fn the_same_frame_seen_on_several_ticks_is_stored_once() {
    let mut log = ResultLog::default();
    assert!(log.observe(Some(&result(10, RequestOrigin::Repeating))));
    assert!(
        !log.observe(Some(&result(10, RequestOrigin::Repeating))),
        "повтор снимка"
    );
    assert!(log.observe(Some(&result(13, RequestOrigin::Repeating))));
    assert!(!log.observe(None));
    assert_eq!(log.repeating_after(None).count(), 2);
}

#[test]
fn trigger_and_burst_frames_move_the_frame_marker_but_never_enter_the_repeating_log() {
    let mut log = ResultLog::default();
    log.observe(Some(&result(10, RequestOrigin::Repeating)));
    log.observe(Some(&result(
        11,
        RequestOrigin::FocusTrigger {
            request: PointRequestId(1),
        },
    )));
    log.observe(Some(&result(
        12,
        RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        },
    )));
    assert_eq!(log.latest_frame(), Some(12));
    assert_eq!(log.latest_repeating().map(|r| r.frame_number.0), Some(10));
    assert_eq!(log.repeating_after(None).count(), 1);
}

#[test]
fn repeating_after_returns_only_strictly_newer_frames() {
    let mut log = ResultLog::default();
    for frame in [1, 4, 7] {
        log.observe(Some(&result(frame, RequestOrigin::Repeating)));
    }
    let after: Vec<i64> = log.repeating_after(Some(4)).map(|r| r.frame_number.0).collect();
    assert_eq!(after, [7]);
}

#[test]
fn log_is_bounded_and_drops_the_oldest() {
    let mut log = ResultLog::default();
    for frame in 0..(RESULT_LOG_CAPACITY as i64 + 5) {
        log.observe(Some(&result(frame, RequestOrigin::Repeating)));
    }
    assert_eq!(log.repeating_after(None).count(), RESULT_LOG_CAPACITY);
    assert_eq!(log.repeating_after(None).next().map(|r| r.frame_number.0), Some(5));
}

#[test]
fn product_median_and_gap_compute_the_documented_values() {
    assert_eq!(
        exposure_product(&result(1, RequestOrigin::Repeating)),
        Some(2_000_000.0 * 400.0)
    );
    let mut without_iso = result(1, RequestOrigin::Repeating);
    without_iso.sensitivity_iso = None;
    assert_eq!(exposure_product(&without_iso), None);
    assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
    assert_eq!(median(vec![4.0, 1.0, 2.0, 3.0]), Some(2.5));
    assert_eq!(median(Vec::new()), None);
    assert_eq!(relative_gap(1.1, 1.0).map(|gap| (gap * 10.0).round()), Some(1.0));
    assert_eq!(relative_gap(1.0, 0.0), None);
}

const CRITERIA: FrozenCriteria = FrozenCriteria {
    tolerance_fraction: 0.01,
    min_tail_samples: 3,
};

#[test]
fn product_frozen_after_frames_counts_from_the_command_frame_to_the_start_of_the_stable_tail() {
    // Догон цели: произведение растёт до кадра 130, дальше стоит.
    let products = [
        (103, 1.0),
        (106, 1.4),
        (112, 1.8),
        (118, 1.95),
        (130, 2.0),
        (133, 2.0),
        (136, 2.001),
        (139, 2.0),
    ];
    assert_eq!(product_frozen_after_frames(100, &products, CRITERIA), Some(30));
}

#[test]
fn product_that_never_changed_is_frozen_from_the_first_frame_after_the_command() {
    let products = [(103, 2.0), (106, 2.0), (109, 2.0)];
    assert_eq!(product_frozen_after_frames(100, &products, CRITERIA), Some(3));
}

#[test]
fn product_still_changing_at_the_end_is_not_frozen() {
    let products = [(103, 1.0), (106, 1.2), (109, 1.4), (112, 1.6), (115, 1.8), (118, 2.0)];
    assert_eq!(product_frozen_after_frames(100, &products, CRITERIA), None);
}

#[test]
fn a_tail_shorter_than_the_minimum_does_not_count_as_frozen() {
    // Стабильны только два последних результата, нужно три.
    let products = [(103, 1.0), (106, 1.5), (109, 2.0), (112, 2.0)];
    assert_eq!(product_frozen_after_frames(100, &products, CRITERIA), None);
}

#[test]
fn a_late_jump_after_a_long_plateau_moves_the_frozen_point_to_the_new_plateau() {
    let products = [
        (103, 2.0),
        (106, 2.0),
        (109, 2.0),
        (112, 3.0),
        (115, 3.0),
        (118, 3.0),
        (121, 3.0),
    ];
    assert_eq!(product_frozen_after_frames(100, &products, CRITERIA), Some(12));
}

#[test]
fn empty_series_is_not_frozen() {
    assert_eq!(product_frozen_after_frames(100, &[], CRITERIA), None);
}
