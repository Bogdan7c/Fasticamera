//! Раздача потребителям: LatestWins, KeepLast(N), NeverDropSilently.

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::FrameSequence;
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;

use super::support::{Rig, passport};
use crate::config::RawFrameStreamConfig;
use crate::stream::{DefectReason, ViewfinderPoll};

const RAW10_4096X3072_BYTES: u64 = 15_728_640;

fn ring_config(budget_bytes: u64, max_frames: u32) -> RawFrameStreamConfig {
    RawFrameStreamConfig {
        snapshot_ring_budget_bytes: budget_bytes,
        snapshot_ring_max_frames: max_frames,
        ..RawFrameStreamConfig::default()
    }
}

/// Кадр сразу со своим результатом: публикуется немедленно.
fn publish_paired(rig: &Rig, timestamp: i64) {
    rig.offer_result(timestamp, 0);
    rig.offer_frame(timestamp, 0);
}

#[test]
fn latest_wins_hands_over_the_newest_frame_and_returns_the_superseded_ones() {
    let rig = Rig::new(RawFrameStreamConfig::default());
    for timestamp in [1_000, 2_000, 3_000] {
        publish_paired(&rig, timestamp);
    }

    let ViewfinderPoll::NewFrame(lease) = rig.stream.viewfinder().poll_latest(rig.at(0)) else {
        panic!("в ячейке должен быть кадр");
    };
    assert_eq!(lease.passport().sequence, FrameSequence(2));
    assert_eq!(lease.passport().sensor_timestamp, SensorTimestamp::from_nanos(3_000));
    assert_eq!(rig.stream.counters().dropped_superseded, 2);
    assert_eq!(rig.log.fence_count(1_000), Some(0));
    assert_eq!(rig.log.fence_count(2_000), Some(0));
    assert!(!rig.log.is_returned(3_000), "выданный кадр ещё у потребителя");

    lease.release_unused();
    assert!(rig.log.is_returned(3_000));
    assert!(matches!(
        rig.stream.viewfinder().poll_latest(rig.at(0)),
        ViewfinderPoll::NoNewFrame
    ));
}

#[test]
fn a_late_published_older_frame_never_replaces_a_newer_one_in_the_slot() {
    let rig = Rig::new(RawFrameStreamConfig::default());
    rig.offer_frame(1_000, 0); // sequence 0, ждёт пару
    rig.offer_frame(2_000, 0); // sequence 1
    rig.offer_result(2_000, 0); // 2000 опубликован раньше 1000
    rig.producer.expire_pending(rig.at(100)); // 1000 уходит по таймауту

    let ViewfinderPoll::NewFrame(lease) = rig.stream.viewfinder().poll_latest(rig.at(100)) else {
        panic!("в ячейке должен быть кадр");
    };
    assert_eq!(lease.passport().sensor_timestamp, SensorTimestamp::from_nanos(2_000));
    assert!(rig.log.is_returned(1_000), "старый опоздавший кадр отпущен");
    assert_eq!(rig.stream.counters().dropped_superseded, 1);
    lease.release_unused();
}

#[test]
fn ring_capacity_follows_budget_frame_size_and_the_frame_cap() {
    let capacity_for = |config: RawFrameStreamConfig, format: RawFormat| {
        let rig = Rig::with_passport(config, passport(format, PixelSize::new(4096, 3072)));
        assert_eq!(
            rig.start.pool.snapshot_ring as usize,
            rig.stream.snapshot_ring().capacity()
        );
        rig.stream.snapshot_ring().capacity()
    };
    assert_eq!(
        capacity_for(ring_config(4 * RAW10_4096X3072_BYTES, 30), RawFormat::Raw10),
        4
    );
    assert_eq!(
        capacity_for(ring_config(4 * RAW10_4096X3072_BYTES + 1, 30), RawFormat::Raw10),
        4
    );
    assert_eq!(
        capacity_for(ring_config(RAW10_4096X3072_BYTES - 1, 30), RawFormat::Raw10),
        0
    );
    assert_eq!(capacity_for(ring_config(0, 30), RawFormat::Raw10), 0);
    assert_eq!(
        capacity_for(ring_config(u64::MAX, 3), RawFormat::Raw10),
        3,
        "потолок кадров"
    );
    // RAW16: 2 байта на пиксель = 25_165_824 байт на кадр.
    assert_eq!(capacity_for(ring_config(2 * 25_165_824, 30), RawFormat::Raw16), 2);
}

#[test]
fn ring_keeps_the_last_n_frames_counts_evictions_and_returns_them_oldest_first() {
    let rig = Rig::new(ring_config(4 * RAW10_4096X3072_BYTES, 30));
    let ring = rig.stream.snapshot_ring();
    let timestamps: Vec<i64> = (1..=6).map(|index| index * 1_000).collect();
    for timestamp in &timestamps {
        publish_paired(&rig, *timestamp);
    }

    assert_eq!(rig.stream.counters().ring_evicted, 2);
    let taken = ring.take_all();
    let order: Vec<i64> = taken
        .iter()
        .map(|lease| lease.passport().sensor_timestamp.as_nanos())
        .collect();
    assert_eq!(
        order,
        vec![3_000, 4_000, 5_000, 6_000],
        "от старых к новым, без вытесненных"
    );
    assert!(rig.log.is_returned(1_000) && rig.log.is_returned(2_000));
    assert!(!rig.log.is_returned(3_000));
    assert!(ring.take_all().is_empty(), "take_all забирает кадры насовсем");
    for lease in taken {
        lease.release_unused();
    }
}

#[test]
fn lossless_consumer_gets_every_frame_in_order_and_defects_when_its_queue_is_full() {
    let mut rig = Rig::new(RawFrameStreamConfig::default());
    let lossless = rig.stream.attach_lossless_consumer(2.try_into().expect("ёмкость"));
    rig.restart();
    assert_eq!(rig.start.pool.lossless, 2);

    for timestamp in [1_000, 2_000, 3_000] {
        publish_paired(&rig, timestamp);
    }

    let defects = lossless.drain_defects();
    assert_eq!(defects.len(), 1);
    assert_eq!(defects[0].sequence, FrameSequence(2));
    assert_eq!(defects[0].sensor_timestamp, SensorTimestamp::from_nanos(3_000));
    assert_eq!(defects[0].reason, DefectReason::QueueFull);
    assert_eq!(rig.stream.counters().lossless_defects, 1);
    assert!(lossless.drain_defects().is_empty(), "события отдаются один раз");

    let first = lossless.take_next().expect("первый кадр в очереди");
    let second = lossless.take_next().expect("второй кадр в очереди");
    assert!(lossless.take_next().is_none());
    assert_eq!(first.passport().sensor_timestamp.as_nanos(), 1_000);
    assert_eq!(second.passport().sensor_timestamp.as_nanos(), 2_000);
    first.release_unused();
    second.release_unused();
}

#[test]
fn each_lossless_consumer_has_its_own_queue() {
    let mut rig = Rig::new(RawFrameStreamConfig::default());
    let small = rig.stream.attach_lossless_consumer(1.try_into().expect("ёмкость"));
    let large = rig.stream.attach_lossless_consumer(4.try_into().expect("ёмкость"));
    rig.restart();
    assert_eq!(rig.start.pool.lossless, 5);
    publish_paired(&rig, 1_000);
    publish_paired(&rig, 2_000);

    assert_eq!(small.drain_defects().len(), 1);
    assert!(large.drain_defects().is_empty());
    let mut taken = Vec::new();
    while let Some(lease) = large.take_next() {
        taken.push(lease);
    }
    assert_eq!(taken.len(), 2);
    for lease in taken.into_iter().chain(small.take_next()) {
        lease.release_unused();
    }
}
