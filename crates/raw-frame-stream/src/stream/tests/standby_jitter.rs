//! Обмен ролей камер и буфер видоискателя с запасом (13b): первый кадр новой показываемой камеры идёт на экран
//! без повторного набора запаса, иначе картинка замирала бы на смене объектива.

use std::sync::Arc;

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;

use super::jitter_buffer::{arrive, tick};
use super::support::{Rig, passport, result_for};
use crate::config::RawFrameStreamConfig;
use crate::stream::{StandbyBegin, StandbyPromotion, StreamEpoch};

fn jitter_rig() -> Rig {
    Rig::new(RawFrameStreamConfig::default())
}

fn begin_standby_epoch(rig: &Rig) -> StreamEpoch {
    let passport: Arc<StreamPassport> = Arc::new(passport(RawFormat::Raw10, PixelSize::new(4096, 3072)));
    match rig.producer.begin_standby_stream(passport) {
        StandbyBegin::Started(start) => start.epoch,
        other => panic!("тёплая эпоха должна начаться, а не {other:?}"),
    }
}

/// Кадр тёплой (после обмена — показываемой) эпохи со своим результатом.
fn arrive_on(rig: &Rig, epoch: StreamEpoch, timestamp: i64) {
    rig.producer
        .offer_capture_result(epoch, result_for(timestamp), rig.at(0));
    rig.producer.offer_frame(epoch, rig.frame(timestamp), rig.at(0));
}

#[test]
fn the_first_frame_of_the_promoted_camera_is_shown_without_refilling_the_reserve() {
    let rig = jitter_rig();
    let standby = begin_standby_epoch(&rig);
    // Старая камера идёт давно: буфер набран, кадры показываются.
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000));
    assert_eq!(tick(&rig), Some(2_000));
    let priming_polls_before = rig.stream.counters().viewfinder_polls_while_priming;

    let promotion = rig.producer.promote_standby(standby);
    assert!(matches!(promotion, StandbyPromotion::Promoted { .. }), "{promotion:?}");
    assert_eq!(tick(&rig), None, "кадров новой камеры ещё нет: показ прежнего");

    // Единственный кадр новой камеры должен выйти на ближайшем тике, а не после `reserve + 1` кадров.
    arrive_on(&rig, standby, 9_000);
    assert_eq!(
        tick(&rig),
        Some(9_000),
        "первый кадр новой камеры без повторного набора запаса"
    );
    let counters = rig.stream.counters();
    assert_eq!(
        counters.viewfinder_polls_while_priming, priming_polls_before,
        "буфер не возвращался в состояние набора"
    );
    assert_eq!((counters.leaked_leases, counters.frames_outstanding), (0, 0));
}

#[test]
fn every_frame_after_the_swap_is_shown_exactly_once_and_accounting_balances() {
    let rig = jitter_rig();
    let standby = begin_standby_epoch(&rig);
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000));
    rig.producer.promote_standby(standby);

    arrive_on(&rig, standby, 10_000);
    arrive_on(&rig, standby, 20_000);
    assert_eq!(tick(&rig), Some(10_000));
    assert_eq!(tick(&rig), Some(20_000));
    assert_eq!(tick(&rig), None, "второго показа одного кадра сенсора нет");

    let counters = rig.stream.counters();
    // Опубликовано 4 доли: 1_000 и 2_000 старой, 10_000 и 20_000 новой; 2_000 сброшен обменом (он лежал в запасе).
    assert_eq!(counters.viewfinder_delivered, 3);
    assert_eq!(counters.viewfinder_flushed, 1);
    assert_eq!(
        counters.frames_published,
        counters.viewfinder_delivered
            + counters.dropped_superseded
            + counters.dropped_jitter_overflow
            + counters.viewfinder_flushed
            + counters.viewfinder_queued,
        "баланс долей кадров ячейки"
    );
    assert_eq!((counters.leaked_leases, counters.frames_outstanding), (0, 0));
}

#[test]
fn a_buffer_that_never_filled_before_the_swap_still_fills_after_it() {
    let rig = jitter_rig();
    let standby = begin_standby_epoch(&rig);
    // Старая камера не успела набрать запас: один кадр, показа не было.
    arrive(&rig, 1_000);
    assert_eq!(tick(&rig), None);
    rig.producer.promote_standby(standby);

    arrive_on(&rig, standby, 10_000);
    assert_eq!(
        tick(&rig),
        None,
        "запас не набирался до обмена — набирается после него, не «выдумывается»"
    );
    arrive_on(&rig, standby, 20_000);
    assert_eq!(tick(&rig), Some(10_000));
}
