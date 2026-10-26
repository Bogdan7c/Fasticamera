//! Тесты темпа на реальных числах сессии 00: 60 к/с = метки через 16 666 667 нс.

use std::time::Duration;

use camera_contract::capture_result::{FrameNumber, ResultKey};
use camera_contract::stream::TargetFrameRate;

use super::*;

const PERIOD_60: i64 = 16_666_667;

fn tracker_for(fps: u32) -> CadenceTracker {
    let mut tracker = CadenceTracker::new(CadenceConfig::default());
    let target = TargetFrameRate::new(fps).expect("частота допустима").frame_duration();
    tracker.begin_session(SessionGeneration(1), target);
    tracker
}

fn complete_meta(frame: i64, timestamp: Option<i64>) -> CaptureResultMeta {
    CaptureResultMeta {
        frame_number: FrameNumber(frame),
        sensor_timestamp: timestamp.map(SensorTimestamp::from_nanos),
        exposure_time: Some(Duration::from_millis(8)),
        sensitivity_iso: Some(100),
        reported_frame_duration: Some(FrameDuration::from_nanos(33_333_333)),
        rolling_shutter_skew: Some(Duration::from_micros(13_660)),
    }
}

fn feed(tracker: &mut CadenceTracker, timestamps: &[i64]) {
    for (index, timestamp) in timestamps.iter().enumerate() {
        tracker.record_completed(&complete_meta(index as i64, Some(*timestamp)));
    }
}

#[test]
fn three_hundred_frames_at_sixty_fps_measure_sixty_with_no_gaps() {
    let mut tracker = tracker_for(60);
    let timestamps: Vec<i64> = (0..300).map(|n| 1_000_000_000 + n * PERIOD_60).collect();
    feed(&mut tracker, &timestamps);
    let stats = tracker.stats();
    assert_eq!(stats.results_completed, 300);
    assert_eq!(stats.intervals, 299);
    assert_eq!(stats.gaps, 0);
    assert_eq!(stats.frames_missed_in_gaps, 0);
    let measured = stats.measured_frames_per_second.expect("интервалы есть");
    assert!((measured - 60.0).abs() < 0.01, "измерено {measured}");
    assert_eq!(stats.min_interval_nanos, Some(PERIOD_60 as u64));
    assert_eq!(stats.max_interval_nanos, Some(PERIOD_60 as u64));
}

#[test]
fn a_hole_of_three_periods_is_one_gap_and_two_lost_frames() {
    let mut tracker = tracker_for(60);
    let mut timestamps: Vec<i64> = (0..10).map(|n| n * PERIOD_60).collect();
    // После кадра 9 следующий пришёл через 3 периода: пропали два кадра.
    timestamps.push(9 * PERIOD_60 + 3 * PERIOD_60);
    feed(&mut tracker, &timestamps);
    let stats = tracker.stats();
    assert_eq!(stats.gaps, 1);
    assert_eq!(stats.frames_missed_in_gaps, 2);
    assert_eq!(stats.max_interval_nanos, Some(3 * PERIOD_60 as u64));
}

#[test]
fn thirty_fps_stream_is_not_reported_as_sixty_and_has_no_false_gaps_against_a_thirty_target() {
    let mut tracker = tracker_for(30);
    let timestamps: Vec<i64> = (0..100).map(|n| n * 33_333_333).collect();
    feed(&mut tracker, &timestamps);
    let measured = tracker.stats().measured_frames_per_second.expect("интервалы есть");
    assert!((measured - 30.0).abs() < 0.01, "измерено {measured}");
    assert_eq!(tracker.stats().gaps, 0);

    // Тот же поток при цели 60 к/с выглядит как сплошные пропуски: по одному кадру на интервал.
    let mut against_sixty = tracker_for(60);
    feed(&mut against_sixty, &timestamps);
    assert_eq!(against_sixty.stats().gaps, 99);
    assert_eq!(against_sixty.stats().frames_missed_in_gaps, 99);
}

#[test]
fn out_of_order_timestamp_is_counted_and_does_not_create_a_false_giant_interval() {
    let mut tracker = tracker_for(60);
    feed(
        &mut tracker,
        &[
            0,
            PERIOD_60,
            2 * PERIOD_60,
            PERIOD_60, /* запоздавший */
            3 * PERIOD_60,
            3 * PERIOD_60, /* повтор */
        ],
    );
    let stats = tracker.stats();
    assert_eq!(stats.timestamps_out_of_order, 2);
    assert_eq!(stats.intervals, 3);
    assert_eq!(stats.gaps, 0);
    assert_eq!(stats.max_interval_nanos, Some(PERIOD_60 as u64));
    assert_eq!(stats.last_timestamp, Some(SensorTimestamp::from_nanos(3 * PERIOD_60)));
}

#[test]
fn missing_timestamp_and_missing_keys_are_counted_per_key() {
    let mut tracker = tracker_for(60);
    let mut meta = complete_meta(1, None);
    meta.exposure_time = None;
    tracker.record_completed(&meta);
    tracker.record_completed(&meta);
    let stats = tracker.stats();
    assert_eq!(stats.results_without_timestamp, 2);
    assert_eq!(stats.intervals, 0);
    let count_of = |key| {
        stats
            .missing_keys
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.results)
    };
    assert_eq!(count_of(ResultKey::SensorTimestamp), Some(2));
    assert_eq!(count_of(ResultKey::ExposureTime), Some(2));
    assert_eq!(count_of(ResultKey::Sensitivity), None);
}

#[test]
fn reported_frame_duration_is_kept_for_the_report_but_does_not_drive_the_rate() {
    let mut tracker = tracker_for(60);
    let timestamps: Vec<i64> = (0..60).map(|n| n * PERIOD_60).collect();
    feed(&mut tracker, &timestamps);
    // HAL врёт 33,3 мс, а темп по меткам — 60.
    assert_eq!(
        tracker.stats().last_reported_frame_duration,
        Some(FrameDuration::from_nanos(33_333_333))
    );
    let measured = tracker.stats().measured_frames_per_second.expect("интервалы есть");
    assert!(measured > 59.9);
}

#[test]
fn capture_failures_and_flushes_are_counted_separately() {
    let mut tracker = tracker_for(60);
    tracker.record_failed(CaptureFailureReason::Error);
    tracker.record_failed(CaptureFailureReason::Flushed);
    tracker.record_failed(CaptureFailureReason::Flushed);
    assert_eq!(tracker.stats().capture_failures, 1);
    assert_eq!(tracker.stats().captures_flushed, 2);
}

#[test]
fn new_session_resets_everything_and_records_session_and_target() {
    let mut tracker = tracker_for(60);
    feed(&mut tracker, &[0, PERIOD_60]);
    tracker.record_failed(CaptureFailureReason::Error);
    tracker.begin_session(
        SessionGeneration(2),
        TargetFrameRate::new(30).expect("30").frame_duration(),
    );
    let stats = tracker.stats();
    assert_eq!(stats.session, Some(SessionGeneration(2)));
    assert_eq!(stats.target_frame_duration, Some(FrameDuration::from_nanos(33_333_333)));
    assert_eq!(stats.results_completed, 0);
    assert_eq!(stats.intervals, 0);
    assert_eq!(stats.capture_failures, 0);
    assert_eq!(stats.first_timestamp, None);
}
