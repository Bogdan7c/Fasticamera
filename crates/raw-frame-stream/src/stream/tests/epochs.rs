//! Эпохи: чужие кадры и результаты, смена и остановка сессии.

use super::support::{Rig, latest_wins_config, result_for};
use crate::config::RawFrameStreamConfig;
use crate::stream::{OfferOutcome, RawFrameStream, ResultOutcome, ViewfinderPoll};

/// Бюджет, при котором RAW10 4096x3072 (15_728_640 байт) даёт кольцо на 3 кадра.
fn config_with_ring() -> RawFrameStreamConfig {
    RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 3 * 15_728_640,
        ..latest_wins_config()
    }
}

#[test]
fn frame_and_result_of_an_old_epoch_are_rejected_and_the_frame_goes_straight_back() {
    let mut rig = Rig::new(latest_wins_config());
    let old_epoch = rig.epoch;
    rig.restart();
    assert_ne!(rig.epoch, old_epoch);

    let outcome = rig.producer.offer_frame(old_epoch, rig.frame(1_000), rig.at(0));
    assert_eq!(outcome, OfferOutcome::StaleEpoch);
    assert_eq!(
        rig.log.fence_count(1_000),
        Some(0),
        "буфер вернулся сразу и без fence-ов"
    );
    let outcome = rig
        .producer
        .offer_capture_result(old_epoch, result_for(1_000), rig.at(0));
    assert_eq!(outcome, ResultOutcome::StaleEpoch);

    let counters = rig.stream.counters();
    assert_eq!((counters.stale_epoch_frames, counters.stale_epoch_results), (1, 1));
    // Чужой результат не должен был спариться с кадром новой эпохи с той же меткой.
    assert_eq!(rig.offer_frame(1_000, 1), OfferOutcome::AwaitingResult);
}

#[test]
fn begin_stream_releases_everything_the_old_epoch_held() {
    let mut rig = Rig::new(config_with_ring());
    // 1_000: опубликован (ячейка + кольцо), 2_000: ждёт пару.
    rig.offer_frame(1_000, 0);
    rig.offer_result(1_000, 0);
    rig.offer_frame(2_000, 0);
    rig.offer_result(9_000, 0); // результат без кадра
    assert_eq!(rig.start.pool.snapshot_ring, 3);

    rig.restart();

    assert!(
        rig.log.is_returned(1_000),
        "доли ячейки и кольца вернулись -> буфер у камеры"
    );
    assert_eq!(rig.log.fence_count(1_000), Some(0));
    assert!(rig.log.is_returned(2_000), "ожидавший пары кадр отпущен");
    let counters = rig.stream.counters();
    assert_eq!(counters.frames_missing_meta_stopped, 1);
    assert_eq!(counters.results_unpaired, 1);
    assert_eq!(counters.frames_outstanding, 0);
    assert!(matches!(
        rig.stream.viewfinder().poll_for_vsync(rig.at(1)),
        ViewfinderPoll::NoNewFrame
    ));
    assert!(rig.stream.snapshot_ring().take_all().is_empty());
}

#[test]
fn end_stream_empties_consumers_and_makes_the_viewfinder_inactive() {
    let rig = Rig::new(config_with_ring());
    rig.offer_frame(1_000, 0);
    rig.offer_result(1_000, 0);
    rig.offer_frame(2_000, 0);

    rig.producer.end_stream(rig.epoch);

    assert!(rig.log.is_returned(1_000));
    assert!(rig.log.is_returned(2_000));
    assert_eq!(rig.stream.counters().frames_missing_meta_stopped, 1);
    assert!(matches!(
        rig.stream.viewfinder().poll_for_vsync(rig.at(1)),
        ViewfinderPoll::StreamInactive
    ));
    assert_eq!(rig.stream.snapshot_ring().capacity(), 0);
    assert_eq!(
        rig.offer_frame(3_000, 2),
        OfferOutcome::StaleEpoch,
        "после остановки кадры не принимаются"
    );
    assert!(rig.log.is_returned(3_000));
}

#[test]
fn end_stream_of_a_previous_epoch_does_not_stop_the_current_one() {
    let mut rig = Rig::new(latest_wins_config());
    let old_epoch = rig.epoch;
    rig.restart();
    rig.producer.end_stream(old_epoch);
    assert_eq!(rig.offer_frame(1_000, 0), OfferOutcome::AwaitingResult);
}

#[test]
fn stream_that_never_began_is_inactive_and_rejects_frames() {
    let stream = RawFrameStream::new(latest_wins_config());
    let now = std::time::Instant::now();
    assert!(matches!(
        stream.viewfinder().poll_for_vsync(now),
        ViewfinderPoll::StreamInactive
    ));
}

#[test]
fn pool_exhaustion_is_counted_for_the_current_epoch_only() {
    let mut rig = Rig::new(latest_wins_config());
    let old_epoch = rig.epoch;
    rig.restart();
    rig.producer.note_pool_exhausted(old_epoch);
    assert_eq!(rig.stream.counters().dropped_pool_exhausted, 0);
    rig.producer.note_pool_exhausted(rig.epoch);
    assert_eq!(rig.stream.counters().dropped_pool_exhausted, 1);
}
