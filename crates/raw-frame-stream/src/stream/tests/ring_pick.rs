//! Выбор кадра из кольца снимков: точное попадание, замены, отказы и учёт буферов.

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::three_a::{BurstId, PointRequestId, RequestOrigin};

use super::support::{Rig, latest_wins_config, result_with_origin};
use crate::config::RawFrameStreamConfig;
use crate::ring_pick::{RingPick, RingPickRequest, RingSubstitutionReason, RingUnavailableReason};

const RAW10_4096X3072_BYTES: u64 = 15_728_640;

fn rig_with_ring(frames: u64) -> Rig {
    Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: frames * RAW10_4096X3072_BYTES,
        ..latest_wins_config()
    })
}

fn stamp(nanos: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(nanos)
}

/// Кадр обычного повторяющегося запроса с метаданными.
fn publish_repeating(rig: &Rig, timestamp: i64) {
    rig.offer_result(timestamp, 0);
    rig.offer_frame(timestamp, 0);
}

fn publish_with_origin(rig: &Rig, timestamp: i64, origin: RequestOrigin) {
    rig.producer
        .offer_capture_result(rig.epoch, result_with_origin(timestamp, origin), rig.at(0));
    rig.offer_frame(timestamp, 0);
}

fn burst_origin() -> RequestOrigin {
    RequestOrigin::Burst {
        burst: BurstId(1),
        index: 0,
    }
}

/// Кадр без метаданных: пара не пришла, ожидание истекло.
fn publish_without_meta(rig: &Rig, timestamp: i64) {
    rig.offer_frame(timestamp, 0);
    rig.producer.expire_pending(rig.at(1_000));
}

/// Метка кадра выданной доли; паникует, если кадра нет.
fn picked_stamp(pick: RingPick) -> (i64, Option<(RingSubstitutionReason, SensorTimestamp)>) {
    match pick {
        RingPick::Exact(lease) => {
            let nanos = lease.passport().sensor_timestamp.as_nanos();
            lease.release_unused();
            (nanos, None)
        }
        RingPick::Substituted {
            lease,
            reason,
            requested,
        } => {
            let nanos = lease.passport().sensor_timestamp.as_nanos();
            lease.release_unused();
            (nanos, Some((reason, requested)))
        }
        RingPick::Unavailable(reason) => panic!("ожидался кадр, а выбор недоступен: {reason:?}"),
    }
}

fn unavailable(pick: RingPick) -> RingUnavailableReason {
    match pick {
        RingPick::Unavailable(reason) => reason,
        other => panic!("ожидалось «недоступно», пришло {other:?}"),
    }
}

#[test]
fn exact_hit_returns_the_shown_frame_and_leaves_the_ring_untouched() {
    let rig = rig_with_ring(4);
    let ring = rig.stream.snapshot_ring();
    for timestamp in [1_000, 2_000, 3_000, 4_000] {
        publish_repeating(&rig, timestamp);
    }

    let (nanos, substitution) = picked_stamp(ring.pick(RingPickRequest::ShownFrame(stamp(3_000))));

    assert_eq!((nanos, substitution), (3_000, None));
    let counters = rig.stream.counters();
    assert_eq!(
        (
            counters.ring_picks_exact,
            counters.ring_picks_substituted,
            counters.ring_picks_unavailable
        ),
        (1, 0, 0)
    );
    assert_eq!(counters.ring_evicted, 0, "выбор не вытесняет кадры");
    let remaining: Vec<i64> = ring
        .take_all()
        .into_iter()
        .map(|lease| {
            let nanos = lease.passport().sensor_timestamp.as_nanos();
            lease.release_unused();
            nanos
        })
        .collect();
    assert_eq!(remaining, vec![1_000, 2_000, 3_000, 4_000], "кадр остался в кольце");
}

#[test]
fn evicted_target_is_replaced_by_the_oldest_eligible_frame_with_the_reason_and_requested_stamp() {
    let rig = rig_with_ring(3);
    for timestamp in [1_000, 2_000, 3_000, 4_000, 5_000] {
        publish_repeating(&rig, timestamp);
    }

    let (nanos, substitution) = picked_stamp(
        rig.stream
            .snapshot_ring()
            .pick(RingPickRequest::ShownFrame(stamp(1_000))),
    );

    assert_eq!(nanos, 3_000, "старейший оставшийся кадр");
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::TargetEvicted, stamp(1_000)))
    );
    assert_eq!(rig.stream.counters().ring_picks_substituted, 1);
}

#[test]
fn target_without_metadata_is_replaced_by_the_nearest_eligible_frame() {
    let rig = rig_with_ring(4);
    publish_repeating(&rig, 1_000);
    publish_without_meta(&rig, 2_000);
    publish_repeating(&rig, 2_600);

    let (nanos, substitution) = picked_stamp(
        rig.stream
            .snapshot_ring()
            .pick(RingPickRequest::ShownFrame(stamp(2_000))),
    );

    assert_eq!(nanos, 2_600, "|2000-2600| = 600 ближе, чем |2000-1000| = 1000");
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::TargetWithoutMeta, stamp(2_000)))
    );
}

#[test]
fn burst_and_focus_trigger_frames_are_never_picked_even_when_they_are_the_exact_target() {
    let rig = rig_with_ring(4);
    publish_repeating(&rig, 1_000);
    publish_with_origin(&rig, 2_000, burst_origin());
    publish_with_origin(
        &rig,
        2_100,
        RequestOrigin::FocusTrigger {
            request: PointRequestId(1),
        },
    );
    publish_repeating(&rig, 3_000);
    let ring = rig.stream.snapshot_ring();

    let (nanos, substitution) = picked_stamp(ring.pick(RingPickRequest::ShownFrame(stamp(2_000))));
    assert_eq!(
        nanos, 1_000,
        "кадры серии и фокус-триггера пропущены; 1000 и 3000 равноудалены от 2000 — берём более ранний"
    );
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::TargetNotRepeating, stamp(2_000)))
    );

    // Оценка 2_100: кадр фокус-триггера (расстояние 0) и кадр серии не берём; 3000 (900) ближе 1000 (1100).
    let (nanos, substitution) = picked_stamp(ring.pick(RingPickRequest::NearestToEstimate(stamp(2_100))));
    assert_eq!(nanos, 3_000);
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::EstimateRequested, stamp(2_100)))
    );
}

#[test]
fn target_missing_from_the_middle_of_the_ring_is_not_called_evicted_and_ties_go_to_the_earlier_frame() {
    let rig = rig_with_ring(4);
    publish_repeating(&rig, 1_000);
    publish_repeating(&rig, 3_000);

    // 2000 лежит ровно посередине: расстояние до обоих 1000 — выигрывает более ранний.
    let (nanos, substitution) = picked_stamp(
        rig.stream
            .snapshot_ring()
            .pick(RingPickRequest::ShownFrame(stamp(2_000))),
    );

    assert_eq!(nanos, 1_000);
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::TargetNotInRing, stamp(2_000)))
    );
}

#[test]
fn estimate_request_never_reports_an_exact_hit_even_on_an_equal_stamp() {
    let rig = rig_with_ring(2);
    publish_repeating(&rig, 1_000);

    let (nanos, substitution) = picked_stamp(
        rig.stream
            .snapshot_ring()
            .pick(RingPickRequest::NearestToEstimate(stamp(1_000))),
    );

    assert_eq!(nanos, 1_000);
    assert_eq!(
        substitution,
        Some((RingSubstitutionReason::EstimateRequested, stamp(1_000)))
    );
    assert_eq!(rig.stream.counters().ring_picks_exact, 0);
}

#[test]
fn disabled_ring_empty_ring_and_no_eligible_frame_are_three_different_refusals() {
    let disabled = Rig::new(latest_wins_config());
    publish_repeating(&disabled, 1_000);
    assert_eq!(
        unavailable(
            disabled
                .stream
                .snapshot_ring()
                .pick(RingPickRequest::ShownFrame(stamp(1_000)))
        ),
        RingUnavailableReason::RingDisabled
    );

    let empty = rig_with_ring(2);
    assert_eq!(
        unavailable(
            empty
                .stream
                .snapshot_ring()
                .pick(RingPickRequest::ShownFrame(stamp(1_000)))
        ),
        RingUnavailableReason::RingEmpty
    );

    let unfit = rig_with_ring(3);
    publish_with_origin(&unfit, 1_000, burst_origin());
    publish_without_meta(&unfit, 2_000);
    assert_eq!(
        unavailable(
            unfit
                .stream
                .snapshot_ring()
                .pick(RingPickRequest::NearestToEstimate(stamp(1_000)))
        ),
        RingUnavailableReason::NoEligibleFrame
    );
    assert_eq!(
        unavailable(
            unfit
                .stream
                .snapshot_ring()
                .pick(RingPickRequest::ShownFrame(stamp(2_000)))
        ),
        RingUnavailableReason::NoEligibleFrame
    );
    assert_eq!(unfit.stream.counters().ring_picks_unavailable, 2);
}

#[test]
fn after_the_stream_ends_the_ring_is_disabled_and_pick_refuses() {
    let rig = rig_with_ring(2);
    publish_repeating(&rig, 1_000);
    rig.producer.end_stream(rig.epoch);

    assert_eq!(
        unavailable(
            rig.stream
                .snapshot_ring()
                .pick(RingPickRequest::ShownFrame(stamp(1_000)))
        ),
        RingUnavailableReason::RingDisabled
    );
}

#[test]
fn picked_share_outlives_ring_eviction_and_the_buffer_returns_exactly_once_after_release() {
    let rig = rig_with_ring(2);
    let ring = rig.stream.snapshot_ring();
    publish_repeating(&rig, 1_000);
    publish_repeating(&rig, 2_000);
    let RingPick::Exact(lease) = ring.pick(RingPickRequest::ShownFrame(stamp(1_000))) else {
        panic!("кадр 1000 есть в кольце");
    };

    // Новый кадр вытесняет 1000 из кольца обычным путём, но доля снимка держит буфер камеры.
    publish_repeating(&rig, 3_000);
    assert_eq!(rig.stream.counters().ring_evicted, 1);
    assert!(!rig.log.is_returned(1_000), "доля снимка держит буфер");

    lease.release_unused();
    assert!(rig.log.is_returned(1_000));
    assert!(rig.log.duplicate_returns().is_empty(), "буфер вернулся ровно один раз");
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0);
    assert_eq!(
        counters.frames_returned, 1,
        "вернулся только 1000; 2000 и 3000 ещё держат кольцо и ячейка"
    );
}

#[test]
fn picking_does_not_touch_other_counters_or_the_consumers_state() {
    let rig = rig_with_ring(3);
    for timestamp in [1_000, 2_000, 3_000] {
        publish_repeating(&rig, timestamp);
    }
    let before = rig.stream.counters();

    let (nanos, _) = picked_stamp(
        rig.stream
            .snapshot_ring()
            .pick(RingPickRequest::ShownFrame(stamp(2_000))),
    );
    assert_eq!(nanos, 2_000);

    let after = rig.stream.counters();
    let expected = crate::RawStreamCounters {
        ring_picks_exact: 1,
        // Доля выдана и возвращена как нетронутая GPU.
        released_unused: before.released_unused + 1,
        ..before
    };
    assert_eq!(after, expected);
    assert_eq!(rig.stream.snapshot_ring().capacity(), 3);
}
