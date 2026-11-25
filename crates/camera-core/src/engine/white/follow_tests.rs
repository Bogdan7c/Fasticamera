//! Чистая часть слежения: темп по меткам, мёртвая зона, блокировка. Сквозные сценарии — `engine/tests/white_follow.rs`.

use super::*;
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};

const PHONE: WhiteSourceId = WhiteSourceId::PHONE;

fn follow(config: WhiteFollowConfig) -> WhiteFollow {
    let mut follow = WhiteFollow::restored(config, WhiteFollowRecord::default(), PHONE);
    assert_eq!(follow.start(PHONE), FollowChange::Changed);
    follow
}

fn at_ms(millis: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(millis * 1_000_000)
}

fn held(origin: WhiteOrigin, mired: f64, duv: f64) -> HeldWhite {
    HeldWhite {
        setting: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(1.0e6 / mired).expect("температура"),
            tint: Tint::from_duv(duv).expect("оттенок"),
        },
        origin,
    }
}

#[test]
fn the_interval_comes_from_the_config_and_counts_from_the_last_question() {
    let mut follow = follow(WhiteFollowConfig {
        ask_interval_ms: 250,
        ..WhiteFollowConfig::default()
    });
    assert_eq!(
        follow.due_source(at_ms(1_000)),
        Some(PHONE),
        "первый кадр спрашивает сразу"
    );
    follow.note_asked(at_ms(1_000));
    assert_eq!(follow.due_source(at_ms(1_249)), None);
    assert_eq!(follow.due_source(at_ms(1_250)), Some(PHONE));
    // Метка раньше прошлой — часы другого потока: ждать, пока они догонят, нельзя.
    assert_eq!(follow.due_source(at_ms(10)), Some(PHONE));
}

#[test]
fn an_out_of_order_frame_does_not_count_as_an_expired_answer() {
    let mut follow = follow(WhiteFollowConfig::default());
    follow.note_asked(at_ms(1_000));
    let timeout = Duration::from_secs(3);
    assert!(!follow.ask_expired(at_ms(990), timeout));
    assert!(!follow.ask_expired(at_ms(3_999), timeout));
    assert!(follow.ask_expired(at_ms(4_000), timeout));
}

#[test]
fn the_dead_zone_numbers_come_from_the_config_and_compare_in_mired() {
    let follow = follow(WhiteFollowConfig {
        dead_zone_mired: 10.0,
        dead_zone_duv: 0.01,
        ..WhiteFollowConfig::default()
    });
    let phone = WhiteOrigin::Source(PHONE);
    let verdict = |to: HeldWhite| follow.verdict(held(phone, 200.0, 0.0), to);
    assert_eq!(verdict(held(phone, 209.9, 0.0099)), FollowVerdict::InsideDeadZone);
    assert_eq!(verdict(held(phone, 190.1, -0.0099)), FollowVerdict::InsideDeadZone);
    assert_eq!(verdict(held(phone, 210.1, 0.0)), FollowVerdict::Move);
    assert_eq!(verdict(held(phone, 200.0, -0.0101)), FollowVerdict::Move);
}

#[test]
fn a_zero_dead_zone_still_rests_on_the_very_same_white() {
    let follow = follow(WhiteFollowConfig {
        dead_zone_mired: 0.0,
        dead_zone_duv: 0.0,
        ..WhiteFollowConfig::default()
    });
    let white = held(WhiteOrigin::Source(PHONE), 200.0, 0.001);
    assert_eq!(follow.verdict(white, white), FollowVerdict::InsideDeadZone);
}

#[test]
fn a_white_of_another_origin_gives_way_to_the_first_answer() {
    let follow = follow(WhiteFollowConfig::default());
    let candidate = held(WhiteOrigin::Source(PHONE), 200.0, 0.0);
    for origin in [
        WhiteOrigin::Manual,
        WhiteOrigin::Fallback,
        WhiteOrigin::Source(WhiteSourceId::new("neural")),
    ] {
        assert_eq!(
            follow.verdict(held(origin, 200.0, 0.0), candidate),
            FollowVerdict::Move,
            "{origin:?}"
        );
    }
}

#[test]
fn the_lock_matters_only_while_following() {
    let mut off = WhiteFollow::restored(WhiteFollowConfig::default(), WhiteFollowRecord::default(), PHONE);
    off.note_lock(AeAfLock::Locked);
    assert!(!off.is_held_by_lock());
    assert_eq!(off.status().state, WhiteFollowState::Off);
    assert_eq!(off.due_source(at_ms(0)), None);

    assert_eq!(off.start(PHONE), FollowChange::Changed);
    assert!(
        off.is_held_by_lock(),
        "включили под блокировкой: режим есть, белая стоит"
    );
    assert_eq!(off.due_source(at_ms(0)), None);
    off.note_lock(AeAfLock::Unlocked);
    assert_eq!(off.due_source(at_ms(0)), Some(PHONE));
}

#[test]
fn following_another_source_is_a_change_and_forgets_the_refusal_of_the_previous_one() {
    let mut follow = follow(WhiteFollowConfig::default());
    follow.note_refused(WhiteRequestOutcome::Busy);
    assert_eq!(follow.start(PHONE), FollowChange::Unchanged);
    assert_eq!(follow.status().last_refusal, Some(WhiteRequestOutcome::Busy));
    let neural = WhiteSourceId::new("neural");
    assert_eq!(follow.start(neural), FollowChange::Changed);
    assert_eq!(follow.status().last_refusal, None);
    assert_eq!(follow.source(), Some(neural));
}

#[test]
fn after_a_flash_the_source_rests_for_the_configured_time_from_the_first_ordinary_frame() {
    let mut follow = follow(WhiteFollowConfig {
        settle_after_flash_ms: 1_000,
        ..WhiteFollowConfig::default()
    });
    let mut due = |tick_ms: i64| {
        follow.note_tick(FollowTick::Frame(at_ms(tick_ms)));
        follow.due_source(at_ms(tick_ms))
    };
    assert_eq!(due(100), Some(PHONE));
    follow.note_tick(FollowTick::FlashInProgress);
    let mut due = |tick_ms: i64| {
        follow.note_tick(FollowTick::Frame(at_ms(tick_ms)));
        follow.due_source(at_ms(tick_ms))
    };
    // Снимок длился 400 мс; успокоение считается от первого обычного кадра (500), а не от начала снимка.
    assert_eq!(due(500), None);
    assert_eq!(due(1_499), None);
    assert_eq!(due(1_500), Some(PHONE));
    assert_eq!(due(1_600), Some(PHONE), "пауза разовая");
}

#[test]
fn releasing_the_lock_does_not_cut_the_flash_pause_but_a_new_stream_does() {
    let mut follow = follow(WhiteFollowConfig::default());
    follow.note_tick(FollowTick::FlashInProgress);
    follow.note_tick(FollowTick::Frame(at_ms(500)));
    follow.note_lock(AeAfLock::Locked);
    follow.note_lock(AeAfLock::Unlocked);
    assert_eq!(follow.due_source(at_ms(500)), None, "автоматика ещё помнит вспышку");
    follow.note_stream_ended();
    assert_eq!(
        follow.due_source(at_ms(10)),
        Some(PHONE),
        "новый поток вспышки не видел"
    );
}
