//! Применяемая белая без раннера и GPU: переходы, побитовая цель, мгновенность при скрытых кадрах.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::white::{WhiteDirective, WhiteTransition};

use super::applied::{AppliedWhite, PROVISIONAL_WHITE};
use super::test_support::{FRAME_NANOS, chromaticity_of, over, setting_of};

fn directive(kelvin: f64, transition: WhiteTransition) -> WhiteDirective {
    WhiteDirective {
        white: setting_of(kelvin),
        transition,
        request: None,
    }
}

fn ts(index: i64) -> i64 {
    1_000_000_000 + index * FRAME_NANOS
}

#[test]
fn without_any_directive_the_white_is_provisional_and_flagged() {
    let mut applied = AppliedWhite::default();
    let frame = applied.advance(ts(0), true);
    assert_eq!((frame.white, frame.provisional), (PROVISIONAL_WHITE, true));
    assert_eq!(applied.target(), None);
}

#[test]
fn the_first_directive_of_a_session_applies_at_once_even_when_it_asks_for_a_slow_transition() {
    let mut applied = AppliedWhite::default();
    applied.advance(ts(0), true);
    applied.accept(&directive(2800.0, over(1000)));
    let frame = applied.advance(ts(1), true);
    assert_eq!(frame.white, chromaticity_of(2800.0), "переезжать не от чего");
    assert!(!frame.provisional);
    assert!(!applied.is_transitioning());
}

#[test]
fn an_over_transition_goes_through_in_between_whites_and_ends_bit_exactly_on_the_target() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(6500.0, WhiteTransition::Immediate));
    applied.advance(ts(0), true);
    applied.accept(&directive(2800.0, over(100)));
    let start = chromaticity_of(6500.0);
    let target = chromaticity_of(2800.0);
    let mut whites = Vec::new();
    for index in 1..=12 {
        whites.push(applied.advance(ts(index), true).white);
    }
    // Первый кадр принял указание: прогресс нулевой, белая прежняя.
    assert_eq!(whites[0], start);
    let between = &whites[1..5];
    for white in between {
        assert_ne!(*white, start);
        assert_ne!(*white, target);
    }
    // Белая идёт монотонно к цели по x (тёплее → x растёт).
    assert!(whites.windows(2).all(|pair| pair[1].x >= pair[0].x));
    // 100 мс ≈ 6 кадров: дальше цель, побитово, и так на всех следующих кадрах.
    for white in &whites[7..] {
        assert_eq!(*white, target);
    }
    assert!(!applied.is_transitioning());
}

#[test]
fn a_stale_or_repeated_sensor_timestamp_makes_no_step() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(6500.0, WhiteTransition::Immediate));
    applied.advance(ts(0), true);
    applied.accept(&directive(2800.0, over(200)));
    applied.advance(ts(1), true);
    let moved = applied.advance(ts(4), true).white;
    assert_ne!(moved, chromaticity_of(6500.0));
    assert_eq!(applied.advance(ts(4), true).white, moved, "та же метка — шага нет");
    assert_eq!(applied.advance(ts(2), true).white, moved, "метка старше — шага нет");
}

#[test]
fn a_directive_with_the_same_white_does_not_restart_the_transition() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(6500.0, WhiteTransition::Immediate));
    applied.advance(ts(0), true);
    applied.accept(&directive(2800.0, over(200)));
    applied.advance(ts(1), true);
    let mid = applied.advance(ts(5), true).white;
    // Сменилась только заявка/способ — белая та же.
    applied.accept(&directive(2800.0, over(5000)));
    applied.accept(&directive(2800.0, WhiteTransition::Immediate));
    let next = applied.advance(ts(6), true).white;
    assert_ne!(next, chromaticity_of(2800.0), "мгновенного скачка нет");
    assert!(next.x >= mid.x, "переход идёт дальше по прежнему расписанию");
}

#[test]
fn a_new_target_mid_transition_starts_from_the_white_applied_now() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(6500.0, WhiteTransition::Immediate));
    applied.advance(ts(0), true);
    applied.accept(&directive(2800.0, over(300)));
    applied.advance(ts(1), true);
    let mid = applied.advance(ts(8), true).white;
    applied.accept(&directive(4000.0, over(300)));
    let first = applied.advance(ts(9), true).white;
    assert_eq!(first, mid, "новый переход стартует от применённой точки");
    for index in 10..40 {
        applied.advance(ts(index), true);
    }
    assert_eq!(applied.advance(ts(40), true).white, chromaticity_of(4000.0));
}

#[test]
fn while_no_frame_is_shown_the_target_is_reached_at_once_whatever_was_asked() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(6500.0, WhiteTransition::Immediate));
    applied.advance(ts(0), true);
    // Холодный старт спрятал кадры; пока их не видно, переезжать плавно незачем.
    applied.advance(ts(1), false);
    applied.accept(&directive(2800.0, over(1000)));
    let frame = applied.advance(ts(2), false);
    assert_eq!(frame.white, chromaticity_of(2800.0));
    // И первый показанный кадр — тоже цель, а не середина.
    assert_eq!(applied.advance(ts(3), true).white, chromaticity_of(2800.0));
}

#[test]
fn the_white_never_changes_without_a_new_directive() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(3300.0, WhiteTransition::Immediate));
    let first = applied.advance(ts(0), true).white;
    for index in 1..200 {
        assert_eq!(applied.advance(ts(index), true).white, first, "кадр {index}");
    }
}

#[test]
fn a_target_the_table_cannot_convert_is_counted_and_the_previous_white_stays() {
    let mut applied = AppliedWhite::default();
    applied.accept(&directive(3300.0, WhiteTransition::Immediate));
    let before = applied.advance(ts(0), true).white;
    applied.accept(&directive(1000.0, WhiteTransition::Immediate));
    assert_eq!(applied.advance(ts(1), true).white, before);
    assert_eq!(applied.unconvertible_targets(), 1);
    assert_eq!(applied.target(), Some(setting_of(3300.0)));
}
