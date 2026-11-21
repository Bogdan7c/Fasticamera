//! Тёплые (standby) эпохи: кадры не показываются, обмен атомарен, `StreamInactive` не возникает.

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;

use super::support::{Rig, frame_number_for, latest_wins_config, passport, result_for};
use crate::config::RawFrameStreamConfig;
use crate::stream::{
    OfferOutcome, RawFrameStream, ResultOutcome, StandbyBegin, StandbyPromotion, StreamEpoch, StreamStart,
    ViewfinderPoll,
};

fn standby_passport() -> Arc<StreamPassport> {
    Arc::new(passport(RawFormat::Raw10, PixelSize::new(4096, 3072)))
}

fn begin_standby(rig: &Rig) -> StreamStart {
    match rig.producer.begin_standby_stream(standby_passport()) {
        StandbyBegin::Started(start) => start,
        other => panic!("тёплая эпоха должна начаться, а не {other:?}"),
    }
}

fn offer_frame_on(rig: &Rig, epoch: StreamEpoch, timestamp: i64, millis: u64) -> OfferOutcome {
    rig.producer.offer_frame(epoch, rig.frame(timestamp), rig.at(millis))
}

fn offer_result_on(rig: &Rig, epoch: StreamEpoch, timestamp: i64, millis: u64) -> ResultOutcome {
    rig.producer
        .offer_capture_result(epoch, result_for(timestamp), rig.at(millis))
}

fn promote(rig: &Rig, standby: StreamEpoch) -> StandbyPromotion {
    rig.producer.promote_standby(standby)
}

/// Забирает кадр видоискателя; паникует, если кадра нет (в т.ч. при `StreamInactive`).
fn take_frame(rig: &Rig, millis: u64) -> crate::RawFrameLease {
    match rig.stream.viewfinder().poll_for_vsync(rig.at(millis)) {
        ViewfinderPoll::NewFrame(lease) => lease,
        other => panic!("ожидался кадр видоискателя, пришло {other:?}"),
    }
}

fn assert_no_new_frame(rig: &Rig, millis: u64) {
    match rig.stream.viewfinder().poll_for_vsync(rig.at(millis)) {
        ViewfinderPoll::NoNewFrame => {}
        other => panic!("ожидалось NoNewFrame, пришло {other:?}"),
    }
}

#[test]
fn standby_frames_go_straight_back_and_never_reach_the_viewfinder_or_ring() {
    let rig = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 3 * 15_728_640,
        ..latest_wins_config()
    });
    let standby = begin_standby(&rig);
    for (index, timestamp) in [1_000, 2_000, 3_000].into_iter().enumerate() {
        let outcome = offer_frame_on(&rig, standby.epoch, timestamp, index as u64);
        assert_eq!(outcome, OfferOutcome::ReturnedAsStandby);
        assert!(rig.log.is_returned(timestamp), "буфер {timestamp} вернулся камере");
        assert_eq!(rig.log.fence_count(timestamp), Some(0), "GPU кадр не видел");
    }
    assert_no_new_frame(&rig, 10);
    assert!(rig.stream.snapshot_ring().take_all().is_empty());
    let counters = rig.stream.counters();
    assert_eq!(counters.standby_frames_returned, 3);
    assert_eq!((counters.frames_published, counters.viewfinder_delivered), (0, 0));
    // Баланс буферов: всё предложенное вернулось через standby-счётчик, лизингов не было.
    assert_eq!(counters.frames_offered, counters.standby_frames_returned);
    assert_eq!((counters.frames_outstanding, counters.leaked_leases), (0, 0));
    assert!(rig.log.duplicate_returns().is_empty());
}

#[test]
fn standby_plan_is_the_full_plan_of_the_shown_stream_and_epochs_never_repeat() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    assert_eq!(standby.pool, rig.start.pool, "тот же паспорт — тот же полный план");
    assert!(standby.epoch > rig.epoch);
    assert_eq!(rig.stream.counters().standby_streams_begun, 1);
    // Показываемую эпоху тёплая не трогает.
    assert_eq!(rig.offer_frame(1_000, 0), OfferOutcome::AwaitingResult);
}

#[test]
fn promotion_shows_the_promoted_camera_paired_with_a_result_that_arrived_before_the_swap() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    // Старая показываемая успела опубликовать кадр в ячейку.
    rig.offer_frame(1_000, 0);
    rig.offer_result(1_000, 0);
    // Результат тёплой пришёл ДО обмена.
    assert_eq!(
        offer_result_on(&rig, standby.epoch, 5_000, 1),
        ResultOutcome::BufferedForStandby
    );

    let promotion = promote(&rig, standby.epoch);
    assert_eq!(
        promotion,
        StandbyPromotion::Promoted {
            shown: standby.epoch,
            demoted_to_standby: Some(rig.epoch),
        }
    );
    // Обмен не вызывает StreamInactive: поток идёт, нового кадра пока нет.
    assert_no_new_frame(&rig, 2);
    assert!(
        rig.log.is_returned(1_000),
        "кадр прежней показываемой не показан, а отпущен"
    );

    // Первый кадр новой показываемой спаривается сразу с результатом, пришедшим до обмена.
    assert_eq!(
        offer_frame_on(&rig, standby.epoch, 5_000, 3),
        OfferOutcome::PublishedPaired
    );
    let lease = take_frame(&rig, 3);
    let details = lease
        .passport()
        .capture
        .details()
        .expect("кадр спарен с ранним результатом");
    assert_eq!(details.result.frame_number, frame_number_for(5_000));
    lease.release_unused();

    let counters = rig.stream.counters();
    assert_eq!((counters.standby_promotions, counters.leaked_leases), (1, 0));
    assert_eq!(counters.frames_outstanding, 0);
}

#[test]
fn frames_of_the_demoted_camera_after_promotion_are_returned_not_shown() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    promote(&rig, standby.epoch);

    // Бывшая показываемая теперь тёплая: её кадры сразу уходят камере.
    assert_eq!(
        offer_frame_on(&rig, rig.epoch, 1_000, 0),
        OfferOutcome::ReturnedAsStandby
    );
    assert!(rig.log.is_returned(1_000));
    assert_no_new_frame(&rig, 1);
    assert_eq!(rig.stream.counters().standby_frames_returned, 1);
    // А кадры новой показываемой доходят до видоискателя.
    offer_frame_on(&rig, standby.epoch, 2_000, 2);
    offer_result_on(&rig, standby.epoch, 2_000, 2);
    let lease = take_frame(&rig, 3);
    assert_eq!(lease.passport().sensor_timestamp, SensorTimestamp::from_nanos(2_000));
    lease.release_unused();
}

#[test]
fn promotion_back_and_forth_twice_keeps_polling_alive_and_shows_the_right_camera() {
    let rig = Rig::new(latest_wins_config());
    let camera_a = rig.epoch;
    let camera_b = begin_standby(&rig).epoch;
    let mut timestamp = 1_000;
    // A показана → B → A → B: после каждого обмена свежий кадр показанной доходит до экрана.
    for (round, (shown, warm)) in [(camera_a, camera_b), (camera_b, camera_a), (camera_a, camera_b)]
        .into_iter()
        .enumerate()
    {
        let millis = round as u64 * 10;
        assert_eq!(
            offer_frame_on(&rig, warm, timestamp, millis),
            OfferOutcome::ReturnedAsStandby
        );
        offer_frame_on(&rig, shown, timestamp + 1, millis);
        offer_result_on(&rig, shown, timestamp + 1, millis);
        let lease = take_frame(&rig, millis + 1);
        assert_eq!(
            lease.passport().sensor_timestamp,
            SensorTimestamp::from_nanos(timestamp + 1),
            "раунд {round}: кадр показанной камеры {shown:?}"
        );
        lease.release_unused();
        let promotion = promote(&rig, warm);
        assert_eq!(
            promotion,
            StandbyPromotion::Promoted {
                shown: warm,
                demoted_to_standby: Some(shown),
            }
        );
        assert_no_new_frame(&rig, millis + 2);
        timestamp += 10;
    }
    let counters = rig.stream.counters();
    assert_eq!(counters.standby_promotions, 3);
    assert_eq!((counters.leaked_leases, counters.frames_outstanding), (0, 0));
    assert!(rig.log.duplicate_returns().is_empty());
}

#[test]
fn promotion_releases_everything_the_old_shown_epoch_held_and_counts_it() {
    let rig = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 3 * 15_728_640,
        ..latest_wins_config()
    });
    let standby = begin_standby(&rig);
    // 1_000: опубликован (ячейка + кольцо), 2_000: ждёт пару.
    rig.offer_frame(1_000, 0);
    rig.offer_result(1_000, 0);
    rig.offer_frame(2_000, 0);

    promote(&rig, standby.epoch);

    assert!(rig.log.is_returned(1_000) && rig.log.is_returned(2_000));
    let counters = rig.stream.counters();
    // Ожидающий кадр + доля ячейки + доля кольца.
    assert_eq!(counters.frames_released_by_promotion, 3);
    assert_eq!(counters.frames_missing_meta_stopped, 1);
    assert_eq!(counters.viewfinder_flushed, 1);
    assert_eq!((counters.frames_outstanding, counters.leaked_leases), (0, 0));
    assert!(rig.stream.snapshot_ring().take_all().is_empty());
    assert_no_new_frame(&rig, 5);
}

#[test]
fn promoting_an_unknown_closed_or_shown_epoch_changes_nothing() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    rig.offer_frame(1_000, 0);
    rig.offer_result(1_000, 0);
    let before = rig.stream.counters();

    for epoch in [StreamEpoch(999), rig.epoch] {
        assert_eq!(promote(&rig, epoch), StandbyPromotion::NoSuchStandby);
    }
    rig.producer.end_standby_stream(standby.epoch);
    assert_eq!(promote(&rig, standby.epoch), StandbyPromotion::NoSuchStandby);

    assert_eq!(rig.stream.counters(), before, "отказ обмена не трогает счётчики");
    let lease = take_frame(&rig, 1);
    assert_eq!(
        lease.passport().sensor_timestamp,
        SensorTimestamp::from_nanos(1_000),
        "прежняя показываемая цела"
    );
    lease.release_unused();
}

#[test]
fn standby_stream_count_is_limited_by_config_and_freed_by_end() {
    let rig = Rig::new(latest_wins_config());
    let first = begin_standby(&rig);
    assert_eq!(
        rig.producer.begin_standby_stream(standby_passport()),
        StandbyBegin::LimitReached { limit: 1 }
    );
    assert_eq!(rig.stream.counters().standby_begins_refused, 1);
    assert_eq!(rig.stream.counters().standby_streams_begun, 1);

    rig.producer.end_standby_stream(first.epoch);
    let second = begin_standby(&rig);
    assert!(second.epoch > first.epoch);

    let roomy = Rig::new(RawFrameStreamConfig {
        max_standby_streams: 2,
        ..latest_wins_config()
    });
    let (a, b) = (begin_standby(&roomy), begin_standby(&roomy));
    assert_ne!(a.epoch, b.epoch);
    assert!(matches!(
        roomy.producer.begin_standby_stream(standby_passport()),
        StandbyBegin::LimitReached { limit: 2 }
    ));

    let disabled = Rig::new(RawFrameStreamConfig {
        max_standby_streams: 0,
        ..latest_wins_config()
    });
    assert_eq!(
        disabled.producer.begin_standby_stream(standby_passport()),
        StandbyBegin::LimitReached { limit: 0 }
    );
}

#[test]
fn ending_a_standby_releases_its_buffered_results_and_makes_the_epoch_stale() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    offer_result_on(&rig, standby.epoch, 5_000, 0);
    offer_result_on(&rig, standby.epoch, 6_000, 0);

    rig.producer.end_standby_stream(standby.epoch);
    assert_eq!(rig.stream.counters().results_unpaired, 2);
    assert_eq!(offer_frame_on(&rig, standby.epoch, 5_000, 1), OfferOutcome::StaleEpoch);
    assert!(rig.log.is_returned(5_000));
    assert_eq!(
        offer_result_on(&rig, standby.epoch, 7_000, 1),
        ResultOutcome::StaleEpoch
    );
    // Повторное закрытие и чужая эпоха — no-op.
    rig.producer.end_standby_stream(standby.epoch);
    rig.producer.end_standby_stream(StreamEpoch(777));
    assert_eq!(rig.stream.counters().results_unpaired, 2);
    // Показываемую закрытие тёплой не затронуло.
    assert_eq!(rig.offer_frame(1_000, 2), OfferOutcome::AwaitingResult);
}

#[test]
fn standby_result_buffer_is_bounded_and_evicted_results_stay_unpaired_after_promotion() {
    let rig = Rig::new(RawFrameStreamConfig {
        max_pending_frames: 2,
        ..latest_wins_config()
    });
    let standby = begin_standby(&rig);
    for timestamp in [1_000, 2_000, 3_000] {
        offer_result_on(&rig, standby.epoch, timestamp, 0);
    }
    assert_eq!(rig.stream.counters().standby_results_evicted, 1);

    promote(&rig, standby.epoch);
    // 1_000 вытеснен: его кадр не получит чужих метаданных, а честно уйдёт с Missing по таймауту.
    assert_eq!(
        offer_frame_on(&rig, standby.epoch, 1_000, 1),
        OfferOutcome::AwaitingResult
    );
    let late = take_frame(&rig, 100);
    assert!(
        late.passport().capture.details().is_none(),
        "вытесненный результат не воскрес"
    );
    late.release_unused();
    assert_eq!(
        offer_frame_on(&rig, standby.epoch, 3_000, 101),
        OfferOutcome::PublishedPaired
    );
    let paired = take_frame(&rig, 102);
    assert_eq!(
        paired.passport().capture.details().expect("спарен").result.frame_number,
        frame_number_for(3_000)
    );
    paired.release_unused();
}

#[test]
fn ending_the_shown_stream_leaves_standby_alone_and_promotion_revives_playback() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    offer_result_on(&rig, standby.epoch, 5_000, 0);

    rig.producer.end_stream(rig.epoch);
    assert!(matches!(
        rig.stream.viewfinder().poll_for_vsync(rig.at(1)),
        ViewfinderPoll::StreamInactive
    ));
    // Тёплая жива: её кадры по-прежнему возвращаются, а не считаются чужими.
    assert_eq!(
        offer_frame_on(&rig, standby.epoch, 4_000, 2),
        OfferOutcome::ReturnedAsStandby
    );

    // Показываемой нет — обмен всё равно работает, прежней показываемой нет.
    assert_eq!(
        promote(&rig, standby.epoch),
        StandbyPromotion::Promoted {
            shown: standby.epoch,
            demoted_to_standby: None,
        }
    );
    assert_no_new_frame(&rig, 3);
    offer_frame_on(&rig, standby.epoch, 5_000, 4);
    let lease = take_frame(&rig, 4);
    assert!(lease.passport().capture.details().is_some());
    lease.release_unused();
    assert_eq!(rig.stream.counters().leaked_leases, 0);
}

#[test]
fn begin_stream_after_promotion_gets_a_fresh_epoch_and_keeps_standby() {
    let mut rig = Rig::new(latest_wins_config());
    let first_shown = rig.epoch;
    let standby = begin_standby(&rig);
    promote(&rig, standby.epoch);

    rig.restart();
    // Номер новой эпохи не совпал ни с выданными тёплой, ни с прежней показываемой.
    assert!(rig.epoch > standby.epoch && rig.epoch > first_shown);
    // Бывшая показываемая (теперь тёплая) пережила перезапуск показываемой.
    assert_eq!(
        offer_frame_on(&rig, first_shown, 1_000, 0),
        OfferOutcome::ReturnedAsStandby
    );
    // Но её бывший партнёр, тоже ушедший при restart, — чужой.
    assert_eq!(offer_frame_on(&rig, standby.epoch, 2_000, 0), OfferOutcome::StaleEpoch);
}

#[test]
fn pool_exhaustion_of_a_standby_is_counted_separately() {
    let rig = Rig::new(latest_wins_config());
    let standby = begin_standby(&rig);
    rig.producer.note_pool_exhausted(standby.epoch);
    rig.producer.note_pool_exhausted(StreamEpoch(999));
    let counters = rig.stream.counters();
    assert_eq!(
        (counters.standby_pool_exhausted, counters.dropped_pool_exhausted),
        (1, 0)
    );
    rig.producer.note_pool_exhausted(rig.epoch);
    assert_eq!(rig.stream.counters().dropped_pool_exhausted, 1);
}

#[test]
fn standby_epochs_are_independent_of_a_fresh_stream_that_never_began() {
    // Тёплую можно начать, даже если показываемой ещё не было (холодный старт).
    let stream = RawFrameStream::new(latest_wins_config());
    let producer = stream.producer();
    let StandbyBegin::Started(start) = producer.begin_standby_stream(standby_passport()) else {
        panic!("тёплая должна начаться без показываемой");
    };
    assert!(matches!(
        stream.viewfinder().poll_for_vsync(std::time::Instant::now()),
        ViewfinderPoll::StreamInactive
    ));
    assert!(matches!(
        producer.promote_standby(start.epoch),
        StandbyPromotion::Promoted {
            demoted_to_standby: None,
            ..
        }
    ));
    assert!(matches!(
        stream.viewfinder().poll_for_vsync(std::time::Instant::now()),
        ViewfinderPoll::NoNewFrame
    ));
}
