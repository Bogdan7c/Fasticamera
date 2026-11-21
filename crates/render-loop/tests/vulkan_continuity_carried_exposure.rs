//! Первый кадр нового потока открывается без удержания, если движок заранее передал ожидаемую экспозицию
//! (сессия 13b, решения 118 и 126) — через НАСТОЯЩИЙ бэкенд показа на реальном драйвере (RADV) и настоящий
//! `RawFrameStream` с атомарным обменом тёплой эпохи (без `StreamInactive`).
//!
//! # Что доказывается
//! * Обмен тёплой камеры: AE нового потока ни разу не мерил, но указание несёт верное ожидание — на первом же
//!   показанном кадре нового потока метка `Live`, ни одного удержанного, скрытого или чёрного кадра, на экране
//!   кадр новой камеры. Контрольный случай БЕЗ ожидания: удержание случается (тест видит нарушение).
//! * Обычное переоткрытие (решение 118): удержание заканчивается по готовности за ≤ 3 скрытых кадра, а не
//!   полным кругом AE или сроком.
//! * Ожидание для ДРУГОЙ камеры или с неправдоподобным уровнем кадр не открывает.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use camera_contract::auto_exposure::{CarriedSceneExposure, ExposureDirective, ExposureProduct};
use camera_contract::fixtures::camera_id;
use continuity_rig::{
    MAIN, Rig, SIZE_A, SIZE_B, WIDE, assert_picture, content_size, finish, gray, passport, reddish, reference_color,
    test_continuity,
};
use raw_frame_stream::{StandbyBegin, StandbyPromotion, StreamEpoch};
use render_loop::backend::DisplayBackend;
use render_loop::config::RevealExposureCheck;
use render_loop::viewfinder::{BlackKind, FrameLabel, TransitionKind};

/// Произведение кадров стенда: 10 мс × ISO 100 = 1 с·ISO.
const FRAME_SECONDS_ISO: f64 = 1.0;

fn carried(camera: &str, seconds_iso: f64) -> ExposureDirective {
    ExposureDirective {
        stream_start: Some(CarriedSceneExposure::for_camera(
            &camera_id(camera),
            ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение"),
        )),
        ..ExposureDirective::DEFAULT
    }
}

/// Стенд с показанным потоком A (главная камера, AE включён, проверка яркости включена).
fn rig_showing_a() -> Option<(Rig, StreamEpoch)> {
    let mut rig = Rig::new(test_continuity(RevealExposureCheck::MeasuredAndPlausible), true)?;
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    Some((rig, epoch_a))
}

/// Обмен тёплой камеры B с показываемой A: атомарно, без `StreamInactive`. Возвращает эпоху B и бывшую A.
fn swap_to_warm_b(rig: &Rig) -> (StreamEpoch, StreamEpoch) {
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let StandbyBegin::Started(start) = rig.stream.producer().begin_standby_stream(passport_b) else {
        panic!("тёплая эпоха не началась");
    };
    match rig.stream.producer().promote_standby(start.epoch) {
        StandbyPromotion::Promoted {
            shown,
            demoted_to_standby: Some(demoted),
        } => (shown, demoted),
        other => panic!("обмен не выполнен: {other:?}"),
    }
}

fn finish_with_standby(rig: &mut Rig, shown: StreamEpoch, demoted: StreamEpoch) {
    rig.stream.producer().end_standby_stream(demoted);
    finish(rig, &[shown]);
}

#[test]
fn a_promoted_warm_camera_is_presented_on_its_first_frame_when_the_engine_carried_the_exposure() {
    let Some((mut rig, epoch_a)) = rig_showing_a() else {
        return;
    };
    // «Движок» ДО обмена объявил: на камере B ожидаем 1 с·ISO (то, с чем идут её кадры).
    rig.exposure_directives.publish(carried(WIDE, FRAME_SECONDS_ISO));
    let (epoch_b, demoted) = swap_to_warm_b(&rig);
    assert_ne!(epoch_a, epoch_b);
    let passport_b = passport(WIDE, SIZE_B);
    let red_ref = reference_color(&passport_b, reddish());

    rig.offer(epoch_b, reddish());
    let first = rig.tick();

    assert_eq!(first.record.label, FrameLabel::Live, "первый кадр B показан сразу");
    assert!(!first.holding);
    assert_picture(
        &rig.live(),
        content_size(SIZE_B),
        red_ref,
        "на экране кадр новой камеры",
    );
    // Ещё несколько кадров B: никаких удержаний и чёрного (собственный замер уже идёт и ничего не ломает).
    for _ in 0..4 {
        rig.offer(epoch_b, reddish());
        assert_eq!(rig.tick().record.label, FrameLabel::Live);
    }
    let counters = rig.counters();
    assert_eq!(counters.holds_started, 0, "{counters:?}");
    assert_eq!(counters.held_frames_presented, 0, "{counters:?}");
    assert_eq!(counters.frames_hidden_by_continuity, 0, "{counters:?}");
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    let auto_exposure = rig
        .backend
        .diagnostics()
        .viewfinder
        .and_then(|viewfinder| viewfinder.auto_exposure)
        .expect("диагностика AE");
    assert!(
        auto_exposure.counters.frames_ready_by_carried_exposure >= 1,
        "первый кадр открыт по ожидаемой экспозиции: {auto_exposure:?}"
    );
    finish_with_standby(&mut rig, epoch_b, demoted);
}

#[test]
fn without_a_carried_exposure_the_same_swap_holds_the_picture() {
    // Контроль: тот же обмен без ожидания — AE нового потока ещё не мерил, яркость «не известна», кадр держится.
    let Some((mut rig, _epoch_a)) = rig_showing_a() else {
        return;
    };
    let (epoch_b, demoted) = swap_to_warm_b(&rig);

    rig.offer(epoch_b, reddish());
    let first = rig.tick();

    assert_eq!(
        first.record.label,
        FrameLabel::Transition(TransitionKind::HeldLastFrame),
        "без ожидания первый кадр B скрыт удержанием"
    );
    let counters = rig.counters();
    assert_eq!(counters.holds_started, 1, "{counters:?}");
    assert!(counters.frames_hidden_by_continuity >= 1, "{counters:?}");
    finish_with_standby(&mut rig, epoch_b, demoted);
}

#[test]
fn a_carried_exposure_for_another_camera_does_not_open_the_frame() {
    let Some((mut rig, _epoch_a)) = rig_showing_a() else {
        return;
    };
    // Ожидание про камеру «main», а кадры идут с «wide»: чужому не верим.
    rig.exposure_directives.publish(carried(MAIN, FRAME_SECONDS_ISO));
    let (epoch_b, demoted) = swap_to_warm_b(&rig);

    rig.offer(epoch_b, reddish());
    assert_eq!(
        rig.tick().record.label,
        FrameLabel::Transition(TransitionKind::HeldLastFrame)
    );
    finish_with_standby(&mut rig, epoch_b, demoted);
}

#[test]
fn an_implausible_carried_exposure_does_not_open_the_frame() {
    let Some((mut rig, _epoch_a)) = rig_showing_a() else {
        return;
    };
    // Ожидали 64 с·ISO, кадры идут с 1 с·ISO: на 6 EV темнее допуска.
    rig.exposure_directives.publish(carried(WIDE, 64.0));
    let (epoch_b, demoted) = swap_to_warm_b(&rig);

    rig.offer(epoch_b, reddish());
    assert_eq!(
        rig.tick().record.label,
        FrameLabel::Transition(TransitionKind::HeldLastFrame)
    );
    finish_with_standby(&mut rig, epoch_b, demoted);
}

/// Обычное переоткрытие: поток A закрылся, B начался заново. Возвращает число скрытых кадров и счётчики.
fn reopen_and_count_hidden_frames(carried_exposure: Option<ExposureDirective>) -> (u64, u64, u64) {
    let (mut rig, epoch_a) = rig_showing_a().expect("стенд");
    rig.stream.producer().end_stream(epoch_a);
    for _ in 0..3 {
        rig.tick();
    }
    if let Some(directive) = carried_exposure {
        rig.exposure_directives.publish(directive);
    }
    let epoch_b = rig.begin(&Arc::new(passport(WIDE, SIZE_B)));
    let ticks = rig.feed_until_live(epoch_b, reddish());
    assert_eq!(ticks.last().map(|tick| tick.record.label), Some(FrameLabel::Live));
    assert!(
        ticks
            .iter()
            .all(|tick| !matches!(tick.record.label, FrameLabel::Black(BlackKind::PictureExpected))),
        "чёрных кадров при ожидаемой картинке нет"
    );
    let counters = rig.counters();
    let result = (
        counters.frames_hidden_by_continuity,
        counters.holds_revealed_by_condition,
        counters.holds_revealed_by_timeout,
    );
    finish(&mut rig, &[epoch_b]);
    result
}

#[test]
fn a_normal_reopen_with_a_carried_exposure_ends_the_hold_by_readiness_within_three_frames() {
    let (hidden, by_condition, by_timeout) = reopen_and_count_hidden_frames(Some(carried(WIDE, FRAME_SECONDS_ISO)));
    assert!(hidden <= 3, "скрыто {hidden} кадров нового потока");
    assert_eq!((by_condition, by_timeout), (1, 0), "удержание кончилось по готовности");
}

#[test]
fn a_normal_reopen_without_a_carried_exposure_waits_longer() {
    // Контроль к тесту выше: без ожидания удержание заметно дольше (полный круг замера AE или срок).
    let (with_hidden, ..) = reopen_and_count_hidden_frames(Some(carried(WIDE, FRAME_SECONDS_ISO)));
    let (without_hidden, ..) = reopen_and_count_hidden_frames(None);
    assert!(
        without_hidden > with_hidden,
        "без ожидания скрыто {without_hidden}, с ожиданием {with_hidden}"
    );
}
