use shell_contract::clock::{BoottimeNanos, MonotonicNanos};

use super::*;

const FRAME_NANOS: i64 = 16_666_667;

fn presented(sequence: u64, index: i64, frame_number: Option<i64>) -> PresentedCameraFrame {
    PresentedCameraFrame {
        sequence,
        sensor_timestamp_nanos: 1_000_000_000 + index * FRAME_NANOS,
        frame_number,
        present_id: u32::try_from(sequence).expect("номер"),
    }
}

// ---- владение метаданными ----

#[test]
fn consistent_series_has_no_violations_even_with_dropped_frames_in_between() {
    let mut tracker = MetadataOwnershipTracker::new(100);
    // Кадры 0, 1, 2, затем потеряны два кадра (3 и 4), показан 5: шаг номера 3 при шаге времени 3 кадра.
    for (sequence, index) in [(0, 0), (1, 1), (2, 2), (3, 5)] {
        tracker.observe(Some(presented(sequence, index, Some(index + 100))));
    }
    let report = tracker.verdict(Some(FRAME_NANOS as f64));
    assert_eq!(report.violations, 0, "{report:?}");
    assert_eq!(report.pairs_checked, 3);
    assert_eq!(report.frames_sampled, 4);
}

#[test]
fn repeated_tick_of_the_same_frame_is_not_counted_twice() {
    let mut tracker = MetadataOwnershipTracker::new(100);
    for _ in 0..5 {
        tracker.observe(Some(presented(7, 3, Some(103))));
    }
    assert_eq!(tracker.verdict(None).frames_sampled, 1);
}

#[test]
fn swapped_frame_numbers_are_caught_as_non_increasing() {
    let mut tracker = MetadataOwnershipTracker::new(100);
    tracker.observe(Some(presented(0, 0, Some(100))));
    // Паспорт чужого кадра: время растёт, а номер вернулся назад.
    tracker.observe(Some(presented(1, 1, Some(99))));
    let report = tracker.verdict(Some(FRAME_NANOS as f64));
    assert_eq!(report.violations, 1);
    assert!(
        report
            .first_violation
            .as_deref()
            .unwrap_or_default()
            .contains("не растёт"),
        "{report:?}"
    );
}

#[test]
fn number_step_that_disagrees_with_the_time_step_is_caught() {
    let mut tracker = MetadataOwnershipTracker::new(100);
    tracker.observe(Some(presented(0, 0, Some(100))));
    // Время прошло на 10 кадров, а номер вырос на 2: результаты склеены с чужими кадрами.
    tracker.observe(Some(presented(1, 10, Some(102))));
    let report = tracker.verdict(Some(FRAME_NANOS as f64));
    assert_eq!(report.violations, 1, "{report:?}");
}

#[test]
fn off_by_one_frame_is_within_tolerance_but_off_by_two_is_not() {
    let within = {
        let mut tracker = MetadataOwnershipTracker::new(100);
        tracker.observe(Some(presented(0, 0, Some(100))));
        tracker.observe(Some(presented(1, 4, Some(105))));
        tracker.verdict(Some(FRAME_NANOS as f64)).violations
    };
    assert_eq!(within, 0);
    let outside = {
        let mut tracker = MetadataOwnershipTracker::new(100);
        tracker.observe(Some(presented(0, 0, Some(100))));
        tracker.observe(Some(presented(1, 4, Some(106))));
        tracker.verdict(Some(FRAME_NANOS as f64)).violations
    };
    assert_eq!(outside, 1);
}

#[test]
fn frames_without_numbers_are_counted_but_not_checked_and_unknown_interval_is_flagged() {
    let mut tracker = MetadataOwnershipTracker::new(100);
    tracker.observe(Some(presented(0, 0, None)));
    tracker.observe(Some(presented(1, 1, Some(101))));
    tracker.observe(None);
    let report = tracker.verdict(None);
    assert_eq!(report.frames_sampled, 2);
    assert_eq!(report.frames_with_frame_number, 1);
    assert_eq!(report.pairs_checked, 0);
    assert!(report.interval_check_skipped);
}

#[test]
fn samples_over_the_limit_are_counted_not_silently_dropped() {
    let mut tracker = MetadataOwnershipTracker::new(2);
    for index in 0..5_i64 {
        tracker.observe(Some(presented(
            u64::try_from(index).expect("индекс"),
            index,
            Some(index),
        )));
    }
    let report = tracker.verdict(Some(FRAME_NANOS as f64));
    assert_eq!(report.samples_over_limit, 3);
    assert_eq!(report.frames_with_frame_number, 5);
}

// ---- задержка ----

fn offset() -> ClockOffset {
    // BOOTTIME = MONOTONIC + 1 с.
    ClockOffset::between(
        BoottimeNanos::from_nanos(11_000_000_000),
        MonotonicNanos::from_nanos(10_000_000_000),
    )
}

fn history(present_id: u32, sensor_nanos: i64) -> PresentedSample {
    PresentedSample {
        present_id,
        sensor_timestamp_nanos: sensor_nanos,
    }
}

fn shown(present_id: u32, monotonic_nanos: i64) -> PastPresentation {
    PastPresentation {
        present_id,
        frame_index: 0,
        actual_present: MonotonicNanos::from_nanos(monotonic_nanos),
    }
}

#[test]
fn latency_is_present_time_converted_to_boottime_minus_sensor_time() {
    let mut sampler = LatencySampler::new(100, 2_000);
    // Кадр снят на BOOTTIME 11,000 с, показан на MONOTONIC 10,040 с = BOOTTIME 11,040 с: 40 мс.
    sampler.collect(&[history(1, 11_000_000_000)], &[shown(1, 10_040_000_000)], offset());
    let report = sampler.report();
    assert_eq!(report.samples, 1);
    assert_eq!(report.p50_ms, Some(40.0));
    assert_eq!(report.max_ms, Some(40.0));
}

#[test]
fn percentiles_follow_nearest_rank() {
    let mut sampler = LatencySampler::new(1000, 2_000);
    // 20 кадров с задержками 1..=20 мс.
    for step in 1..=20_i64 {
        let sensor = 11_000_000_000 + step * 20_000_000;
        let present_monotonic = 10_000_000_000 + step * 20_000_000 + step * 1_000_000;
        sampler.collect(
            &[history(u32::try_from(step).expect("шаг"), sensor)],
            &[shown(u32::try_from(step).expect("шаг"), present_monotonic)],
            offset(),
        );
    }
    let report = sampler.report();
    assert_eq!(report.samples, 20);
    assert_eq!(report.p50_ms, Some(10.0));
    assert_eq!(report.p95_ms, Some(19.0));
    assert_eq!(report.max_ms, Some(20.0));
}

#[test]
fn the_same_frame_is_never_counted_twice_across_ticks() {
    let mut sampler = LatencySampler::new(100, 2_000);
    let frames = [history(1, 11_000_000_000)];
    let times = [shown(1, 10_030_000_000)];
    for _ in 0..4 {
        sampler.collect(&frames, &times, offset());
    }
    assert_eq!(sampler.report().samples, 1);
}

#[test]
fn a_frame_waits_for_its_present_time_and_is_abandoned_when_a_newer_one_matched() {
    let mut sampler = LatencySampler::new(100, 2_000);
    // Кадр 1 поставлен на показ, времени показа ещё нет: ждёт.
    sampler.collect(&[history(1, 11_000_000_000)], &[], offset());
    assert_eq!(sampler.report().samples, 0);
    assert_eq!(sampler.report().unmatched_presentations, 0);
    // Время кадра 1 пришло — учтён.
    sampler.collect(&[history(1, 11_000_000_000)], &[shown(1, 10_030_000_000)], offset());
    assert_eq!(sampler.report().samples, 1);
    // Кадр 2 так и не получил времени, а кадр 3 получил: кадр 2 брошен и посчитан.
    sampler.collect(
        &[history(2, 11_016_000_000), history(3, 11_033_000_000)],
        &[shown(3, 10_070_000_000)],
        offset(),
    );
    let report = sampler.report();
    assert_eq!(report.samples, 2);
    assert_eq!(report.unmatched_presentations, 1);
}

#[test]
fn nonsense_latencies_are_counted_as_implausible_not_measured() {
    let mut sampler = LatencySampler::new(100, 100);
    // Показ раньше экспозиции (отрицательная) и спустя 5 с (выше порога 100 мс).
    sampler.collect(
        &[history(1, 11_000_000_000), history(2, 12_000_000_000)],
        &[shown(1, 9_990_000_000), shown(2, 15_000_000_000)],
        offset(),
    );
    let report = sampler.report();
    assert_eq!(report.samples, 0);
    assert_eq!(report.implausible, 2);
    assert_eq!(report.p50_ms, None);
}

#[test]
fn samples_over_the_limit_and_clock_failures_are_visible() {
    let mut sampler = LatencySampler::new(1, 2_000);
    sampler.collect(
        &[history(1, 11_000_000_000), history(2, 11_016_000_000)],
        &[shown(1, 10_030_000_000), shown(2, 10_050_000_000)],
        offset(),
    );
    sampler.note_clock_failure(&crate::clock_offset::ClockReadError {
        clock: "CLOCK_BOOTTIME",
        errno: 22,
    });
    let report = sampler.report();
    assert_eq!(report.samples, 1);
    assert_eq!(report.dropped_over_limit, 1);
    assert_eq!(report.clock_read_failures, 1);
    assert!(report.last_clock_error.unwrap_or_default().contains("CLOCK_BOOTTIME"));
}

#[test]
fn frames_already_on_show_are_skipped_even_if_their_present_time_arrives_later() {
    let mut sampler = LatencySampler::new(100, 2_000);
    // Кадр 1 поставлен на показ до начала окна (время показа от драйвера ещё не пришло).
    sampler.skip_already_presented(&[history(1, 11_000_000_000)]);
    // Время показа кадра 1 пришло, и показан новый кадр 2 — в окно попадает только кадр 2.
    sampler.collect(
        &[history(1, 11_000_000_000), history(2, 11_016_000_000)],
        &[shown(1, 10_040_000_000), shown(2, 10_046_000_000)],
        offset(),
    );
    let report = sampler.report();
    assert_eq!(report.samples, 1);
    assert_eq!(report.p50_ms, Some(30.0));
    // Пустая история ничего не меняет.
    let mut fresh = LatencySampler::new(100, 2_000);
    fresh.skip_already_presented(&[]);
    fresh.collect(&[history(1, 11_000_000_000)], &[shown(1, 10_040_000_000)], offset());
    assert_eq!(fresh.report().samples, 1);
}
