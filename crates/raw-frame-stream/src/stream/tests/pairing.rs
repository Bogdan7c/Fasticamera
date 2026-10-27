//! Спаривание кадра и результата по точному `SENSOR_TIMESTAMP`.

use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, FrameSequence, MissingCaptureMeta};

use super::support::{Rig, frame_number_for, latest_wins_config};
use crate::config::RawFrameStreamConfig;
use crate::stream::{OfferOutcome, ResultOutcome, ViewfinderPoll};

fn config_with_room() -> RawFrameStreamConfig {
    RawFrameStreamConfig {
        max_pending_frames: 8,
        ..latest_wins_config()
    }
}

#[test]
fn frame_then_result_publishes_the_frame_with_its_own_metadata_and_records_the_wait() {
    let rig = Rig::new(latest_wins_config());
    let viewfinder = rig.stream.viewfinder();

    assert_eq!(rig.offer_frame(5_000, 0), OfferOutcome::AwaitingResult);
    assert!(matches!(
        viewfinder.poll_for_vsync(rig.at(1)),
        ViewfinderPoll::NoNewFrame
    ));
    assert_eq!(rig.offer_result(5_000, 5), ResultOutcome::PairedWithFrame);

    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_for_vsync(rig.at(6)) else {
        panic!("после пары кадр должен быть в ячейке");
    };
    let passport = lease.passport();
    assert_eq!(passport.sensor_timestamp, SensorTimestamp::from_nanos(5_000));
    let details = passport.capture.details().expect("кадр спарен");
    assert_eq!(details.result.frame_number, frame_number_for(5_000));
    assert_eq!(rig.stream.counters().max_pairing_wait_micros, 5_000);
    assert_eq!(rig.stream.counters().frames_paired, 1);
    lease.release_unused();
}

#[test]
fn result_then_frame_publishes_immediately_with_zero_wait() {
    let rig = Rig::new(latest_wins_config());
    let viewfinder = rig.stream.viewfinder();

    assert_eq!(rig.offer_result(7_000, 0), ResultOutcome::Buffered);
    assert_eq!(rig.offer_frame(7_000, 30), OfferOutcome::PublishedPaired);

    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_for_vsync(rig.at(30)) else {
        panic!("кадр должен быть в ячейке");
    };
    let details = lease.passport().capture.details().expect("кадр спарен");
    assert_eq!(details.result.frame_number, frame_number_for(7_000));
    let counters = rig.stream.counters();
    assert_eq!((counters.frames_paired, counters.total_pairing_wait_micros), (1, 0));
    lease.release_unused();
}

#[test]
fn shuffled_frames_and_results_each_get_exactly_their_own_metadata() {
    let mut rig = Rig::new(config_with_room());
    let consumer = rig.stream.attach_lossless_consumer(16.try_into().expect("ёмкость"));
    // Новая эпоха, чтобы очередь потребителя вошла в план (на раздачу не влияет).
    rig.restart();

    // 20 событий (кадр и результат на каждую метку), перемешанные внутри окон по 4.
    let mut events: Vec<(bool, i64)> = (1..=10_i64)
        .flat_map(|index| [(true, index * 1_000), (false, index * 1_000)])
        .collect();
    let mut seed = 0x9E37_79B9_u64;
    for window in events.chunks_mut(4) {
        for position in (1..window.len()).rev() {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let other = usize::try_from((seed >> 33) % (position as u64 + 1)).expect("индекс");
            window.swap(position, other);
        }
    }
    for (is_frame, timestamp) in events {
        if is_frame {
            rig.offer_frame(timestamp, 0);
        } else {
            rig.offer_result(timestamp, 0);
        }
    }

    let mut seen = Vec::new();
    while let Some(lease) = consumer.take_next() {
        let passport = lease.passport();
        let details = passport.capture.details().expect("все 10 кадров спарены");
        assert_eq!(details.result.sensor_timestamp, Some(passport.sensor_timestamp));
        assert_eq!(
            details.result.frame_number,
            frame_number_for(passport.sensor_timestamp.as_nanos())
        );
        seen.push(passport.sensor_timestamp.as_nanos());
        lease.release_unused();
    }
    seen.sort_unstable();
    assert_eq!(seen, (1..=10).map(|index| index * 1_000).collect::<Vec<_>>());
    assert_eq!(rig.stream.counters().frames_missing_meta_timeout, 0);
}

#[test]
fn timestamps_that_differ_by_one_nanosecond_never_pair() {
    let rig = Rig::new(latest_wins_config());
    assert_eq!(rig.offer_frame(10_000, 0), OfferOutcome::AwaitingResult);
    assert_eq!(rig.offer_result(10_001, 1), ResultOutcome::Buffered);
    assert_eq!(rig.stream.counters().frames_paired, 0);
}

#[test]
fn lost_result_publishes_missing_exactly_at_the_timeout_and_not_before() {
    let rig = Rig::new(latest_wins_config());
    let viewfinder = rig.stream.viewfinder();
    rig.offer_frame(9_000, 0);

    assert!(matches!(
        viewfinder.poll_for_vsync(rig.at(49)),
        ViewfinderPoll::NoNewFrame
    ));
    assert_eq!(rig.stream.counters().frames_missing_meta_timeout, 0);

    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_for_vsync(rig.at(50)) else {
        panic!("по таймауту кадр должен уйти без метаданных");
    };
    assert_eq!(
        lease.passport().capture.details(),
        None,
        "чужие метаданные подставлять нельзя"
    );
    assert!(matches!(
        lease.passport().capture,
        camera_contract::frame_passport::FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::PairingTimedOut
        }
    ));
    assert_eq!(rig.stream.counters().frames_missing_meta_timeout, 1);
    lease.release_unused();
}

#[test]
fn lost_frame_leaves_its_result_unpaired_once_the_buffer_evicts_it() {
    let rig = Rig::new(RawFrameStreamConfig {
        max_buffered_results: 2,
        ..latest_wins_config()
    });
    for timestamp in [1_000, 2_000, 3_000] {
        assert_eq!(rig.offer_result(timestamp, 0), ResultOutcome::Buffered);
    }
    assert_eq!(rig.stream.counters().results_unpaired, 1, "результат 1000 вытеснен");

    // Кадр вытесненного результата пары уже не найдёт, кадр оставшегося — найдёт.
    assert_eq!(rig.offer_frame(1_000, 1), OfferOutcome::AwaitingResult);
    assert_eq!(rig.offer_frame(3_000, 2), OfferOutcome::PublishedPaired);
}

#[test]
fn result_without_timestamp_is_counted_and_never_buffered() {
    let rig = Rig::new(latest_wins_config());
    let nameless = FrameCaptureDetails {
        result: CaptureResultMeta {
            frame_number: FrameNumber(1),
            sensor_timestamp: None,
            exposure_time: None,
            sensitivity_iso: None,
            reported_frame_duration: None,
            rolling_shutter_skew: None,
        },
        color: CaptureColorHints::default(),
    };
    let outcome = rig.producer.offer_capture_result(rig.epoch, nameless, rig.at(0));
    assert_eq!(outcome, ResultOutcome::WithoutTimestamp);
    assert_eq!(rig.stream.counters().results_unpaired, 1);
}

#[test]
fn pending_overflow_publishes_the_oldest_frame_early_without_metadata() {
    let rig = Rig::new(latest_wins_config()); // max_pending_frames = 2
    let viewfinder = rig.stream.viewfinder();
    rig.offer_frame(1_000, 0);
    rig.offer_frame(2_000, 1);
    assert_eq!(rig.stream.counters().frames_published, 0);

    assert_eq!(rig.offer_frame(3_000, 2), OfferOutcome::AwaitingResult);
    assert_eq!(rig.stream.counters().frames_published, 1);
    assert_eq!(rig.stream.counters().frames_missing_meta_timeout, 1);

    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_for_vsync(rig.at(2)) else {
        panic!("вытесненный из ожидания кадр должен быть в ячейке");
    };
    assert_eq!(lease.passport().sensor_timestamp, SensorTimestamp::from_nanos(1_000));
    assert_eq!(lease.passport().sequence, FrameSequence(0));
    assert_eq!(lease.passport().capture.details(), None);
    lease.release_unused();
    // Пара для переживших кадров всё ещё находится.
    assert_eq!(rig.offer_result(2_000, 3), ResultOutcome::PairedWithFrame);
}
