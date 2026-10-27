use super::*;

#[test]
fn fresh_counters_are_all_zero_and_have_no_last_error() {
    assert_eq!(
        ReaderCounters::default().snapshot(),
        ReaderSnapshot {
            frames_acquired: 0,
            acquire_errors: 0,
            last_acquire_error: None,
            pool_exhausted: 0,
            fence_merge_failures: 0,
            images_returned: 0,
            images_leaked: 0,
            readers_retired: 0,
            readers_deleted: 0,
            readers_leaked: 0,
            cpu_access_probe_copies: 0,
            cpu_access_probe_failures: 0,
            arrival_age: ArrivalAgeSnapshot::default(),
        }
    );
}

#[test]
fn each_event_moves_only_its_own_counter() {
    let counters = ReaderCounters::default();
    for _ in 0..3 {
        counters.record_frame_acquired();
    }
    counters.record_acquire_error(-30002);
    counters.record_acquire_error(-10000);
    counters.record_pool_exhausted();
    counters.record_fence_merge_failure();
    counters.record_image_returned();
    counters.record_image_returned();
    counters.record_image_leaked();
    counters.record_reader_retired();
    counters.record_reader_deleted();
    counters.record_reader_leaked();
    counters.record_cpu_probe_copy();
    counters.record_cpu_probe_failure();
    counters.record_cpu_probe_failure();
    counters.record_arrival_age(Duration::from_micros(40_000));
    counters.record_arrival_age(Duration::from_micros(20_000));
    counters.record_arrival_clock_failure();
    counters.record_arrival_age_negative();
    counters.record_arrival_age_negative();
    assert_eq!(
        counters.snapshot(),
        ReaderSnapshot {
            frames_acquired: 3,
            acquire_errors: 2,
            last_acquire_error: Some(-10000),
            pool_exhausted: 1,
            fence_merge_failures: 1,
            images_returned: 2,
            images_leaked: 1,
            readers_retired: 1,
            readers_deleted: 1,
            readers_leaked: 1,
            cpu_access_probe_copies: 1,
            cpu_access_probe_failures: 2,
            arrival_age: ArrivalAgeSnapshot {
                samples: 2,
                total_micros: 60_000,
                min_micros: Some(20_000),
                max_micros: Some(40_000),
                clock_failures: 1,
                negative: 2,
            },
        }
    );
}

#[test]
fn arrival_age_mean_is_none_without_samples_and_exact_with_them() {
    assert_eq!(ArrivalAgeSnapshot::default().mean_micros(), None);
    let counters = ReaderCounters::default();
    counters.record_arrival_age(Duration::from_micros(10));
    counters.record_arrival_age(Duration::from_micros(31));
    assert_eq!(counters.snapshot().arrival_age.mean_micros(), Some(20));
}
