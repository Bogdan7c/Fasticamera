use camera_android::reader_counters::ArrivalAgeSnapshot;

use super::*;

#[test]
fn stream_delta_subtracts_cumulative_counters_and_keeps_peaks_and_gauges() {
    let before = RawStreamCounters {
        frames_offered: 100,
        cpu_copies: 4,
        leaked_leases: 1,
        max_frames_outstanding: 3,
        frames_outstanding: 2,
        max_pairing_wait_micros: 900,
        ..RawStreamCounters::default()
    };
    let after = RawStreamCounters {
        frames_offered: 460,
        cpu_copies: 4,
        leaked_leases: 1,
        max_frames_outstanding: 5,
        frames_outstanding: 1,
        max_pairing_wait_micros: 1200,
        ..RawStreamCounters::default()
    };
    let delta = stream_delta(&before, &after);
    assert_eq!(delta.frames_offered, 360);
    assert_eq!(delta.cpu_copies, 0, "копии прошлых сценариев не засчитываются этому");
    assert_eq!(
        delta.leaked_leases, 0,
        "старая утечка не перекладывается на новый прогон"
    );
    assert_eq!(delta.max_frames_outstanding, 5, "пик берётся как есть");
    assert_eq!(delta.frames_outstanding, 1);
    assert_eq!(delta.max_pairing_wait_micros, 1200);
}

#[test]
fn stream_delta_counts_ring_picks_of_this_run_only() {
    let before = RawStreamCounters {
        ring_picks_exact: 3,
        ring_picks_substituted: 1,
        ring_picks_unavailable: 2,
        ..RawStreamCounters::default()
    };
    let after = RawStreamCounters {
        ring_picks_exact: 10,
        ring_picks_substituted: 1,
        ring_picks_unavailable: 5,
        ..RawStreamCounters::default()
    };
    let delta = stream_delta(&before, &after);
    assert_eq!(
        (
            delta.ring_picks_exact,
            delta.ring_picks_substituted,
            delta.ring_picks_unavailable
        ),
        (7, 0, 3)
    );
}

#[test]
fn counters_that_went_backwards_saturate_at_zero() {
    let before = RawStreamCounters {
        frames_offered: 10,
        ..RawStreamCounters::default()
    };
    assert_eq!(stream_delta(&before, &RawStreamCounters::default()).frames_offered, 0);
}

#[test]
fn reader_delta_subtracts_every_counter_and_keeps_the_last_error_code() {
    let before = ReaderSnapshot {
        frames_acquired: 10,
        acquire_errors: 1,
        last_acquire_error: Some(-5),
        pool_exhausted: 0,
        fence_merge_failures: 0,
        images_returned: 9,
        images_leaked: 0,
        readers_retired: 1,
        readers_deleted: 1,
        readers_leaked: 0,
        cpu_access_probe_copies: 0,
        cpu_access_probe_failures: 0,
        arrival_age: ArrivalAgeSnapshot {
            samples: 10,
            total_micros: 400_000,
            min_micros: Some(30_000),
            max_micros: Some(50_000),
            clock_failures: 1,
            negative: 0,
        },
    };
    let after = ReaderSnapshot {
        frames_acquired: 70,
        acquire_errors: 3,
        last_acquire_error: Some(-9),
        pool_exhausted: 2,
        fence_merge_failures: 1,
        images_returned: 66,
        images_leaked: 1,
        readers_retired: 2,
        readers_deleted: 2,
        readers_leaked: 1,
        cpu_access_probe_copies: 30,
        cpu_access_probe_failures: 4,
        arrival_age: ArrivalAgeSnapshot {
            samples: 70,
            total_micros: 2_800_000,
            min_micros: Some(25_000),
            max_micros: Some(90_000),
            clock_failures: 3,
            negative: 2,
        },
    };
    let delta = reader_delta(&before, &after);
    assert_eq!(delta.frames_acquired, 60);
    assert_eq!(delta.acquire_errors, 2);
    assert_eq!(delta.last_acquire_error, Some(-9));
    assert_eq!(delta.pool_exhausted, 2);
    assert_eq!(delta.images_returned, 57);
    assert_eq!(delta.images_leaked, 1);
    assert_eq!(delta.readers_leaked, 1);
    assert_eq!(delta.cpu_access_probe_copies, 30);
    assert_eq!(delta.cpu_access_probe_failures, 4);
    assert_eq!(
        delta.arrival_age,
        ArrivalAgeSnapshot {
            samples: 60,
            total_micros: 2_400_000,
            // min/max — за всё время процесса, берутся из «после».
            min_micros: Some(25_000),
            max_micros: Some(90_000),
            clock_failures: 2,
            negative: 2,
        }
    );
}

#[test]
fn render_delta_counts_only_what_happened_during_the_run() {
    let before = CameraStreamDiagnostics {
        camera_frames_presented: 500,
        release_export_failures: 2,
        ..CameraStreamDiagnostics::default()
    };
    let after = CameraStreamDiagnostics {
        camera_frames_presented: 3_700,
        release_export_failures: 2,
        import_cache_misses: 4,
        ..CameraStreamDiagnostics::default()
    };
    let delta = RenderCameraDelta::between(&before, &after);
    assert_eq!(delta.camera_frames_presented, 3_200);
    assert_eq!(delta.release_export_failures, 0);
    assert_eq!(delta.import_cache_misses, 4);
}
