#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Элементы «внимание»: мерцание ожидания и тряска отказа. Чистая логика от времени: значения по моментам,
//! признак «ещё движется» и поведение при Reduce Motion.

use ui_core::time::MotionPreference;
use ui_kit::attention::{RefusalShake, WaitingFlicker};
use ui_kit::tokens::{AttentionTokens, TokenError, UiKitTokens};

mod common;
use common::ms;

fn tokens() -> AttentionTokens {
    AttentionTokens::default()
}

// ---- мерцание ожидания ----

#[test]
fn idle_flicker_is_fully_opaque_and_not_animating() {
    let flicker = WaitingFlicker::new(&tokens());
    assert_eq!(flicker.opacity(ms(0)), 1.0);
    assert!(!flicker.is_animating(ms(0)));
    assert!(!flicker.is_waiting());
}

#[test]
fn waiting_flicker_swings_smoothly_between_one_and_the_floor_with_a_one_second_period() {
    let tokens = tokens();
    let mut flicker = WaitingFlicker::new(&tokens);
    flicker.start(ms(0));
    assert!(flicker.is_waiting() && flicker.is_animating(ms(0)));
    assert!(
        (flicker.opacity(ms(0)) - 1.0).abs() < 1e-9,
        "начинает с полной прозрачности"
    );
    assert!(
        (flicker.opacity(ms(500)) - tokens.waiting_min_opacity).abs() < 1e-9,
        "через полпериода — нижний уровень"
    );
    assert!((flicker.opacity(ms(1000)) - 1.0).abs() < 1e-9, "через период — снова 1");
    // Всегда внутри диапазона и без скачков: соседние кадры (16 мс) отличаются мало.
    let mut previous = flicker.opacity(ms(0));
    for millisecond in (16..=3000).step_by(16) {
        let opacity = flicker.opacity(ms(millisecond));
        assert!(
            (tokens.waiting_min_opacity - 1e-9..=1.0 + 1e-9).contains(&opacity),
            "{millisecond} мс: {opacity}"
        );
        assert!(
            (opacity - previous).abs() < 0.12,
            "скачок на {millisecond} мс: {previous} → {opacity}"
        );
        previous = opacity;
    }
    assert!(flicker.is_animating(ms(10_000)), "пока ждём — кадры нужны");
}

#[test]
fn restarting_while_waiting_does_not_reset_the_wave() {
    let mut flicker = WaitingFlicker::new(&tokens());
    flicker.start(ms(0));
    let before = flicker.opacity(ms(300));
    flicker.start(ms(300));
    assert_eq!(flicker.opacity(ms(300)), before, "повторный start не сбросил волну в 1");
}

#[test]
fn stopping_returns_to_full_opacity_softly_and_ends_the_animation() {
    let tokens = tokens();
    let mut flicker = WaitingFlicker::new(&tokens);
    flicker.start(ms(0));
    flicker.stop(ms(500)); // в самом тёмном месте волны
    assert!(!flicker.is_waiting());
    assert!(
        (flicker.opacity(ms(500)) - tokens.waiting_min_opacity).abs() < 1e-9,
        "без скачка в момент остановки"
    );
    let mut previous = flicker.opacity(ms(500));
    for millisecond in (516..=900).step_by(16) {
        let opacity = flicker.opacity(ms(millisecond));
        assert!(opacity >= previous - 1e-12, "возврат монотонен: {previous} → {opacity}");
        previous = opacity;
    }
    assert!((flicker.opacity(ms(900)) - 1.0).abs() < 1e-9);
    assert!(flicker.is_animating(ms(600)), "пока возвращается — кадры нужны");
    assert!(!flicker.is_animating(ms(800)), "вернулась — кадры больше не нужны");
}

#[test]
fn stop_without_waiting_does_nothing_and_start_during_settling_has_no_jump() {
    let mut flicker = WaitingFlicker::new(&tokens());
    flicker.stop(ms(0));
    assert_eq!(flicker.opacity(ms(10)), 1.0);
    assert!(!flicker.is_animating(ms(10)));

    flicker.start(ms(0));
    flicker.stop(ms(500));
    let mid_settle = flicker.opacity(ms(600));
    assert!(mid_settle < 1.0 - 1e-6, "возврат ещё идёт");
    flicker.start(ms(600));
    assert!(
        (flicker.opacity(ms(600)) - mid_settle).abs() < 1e-9,
        "новое ожидание началось с текущего уровня"
    );
    assert!(flicker.is_waiting());
}

#[test]
fn time_going_backwards_keeps_the_flicker_at_its_start() {
    let mut flicker = WaitingFlicker::new(&tokens());
    flicker.start(ms(1000));
    assert!((flicker.opacity(ms(900)) - 1.0).abs() < 1e-9);
}

// ---- тряска отказа ----

#[test]
fn shake_swings_with_decaying_amplitude_for_about_four_tenths_of_a_second() {
    let tokens = tokens();
    let mut shake = RefusalShake::new(&tokens, MotionPreference::Full);
    assert_eq!(shake.offset_x_pt(ms(0)), 0.0);
    assert!(!shake.is_animating(ms(0)));
    shake.trigger(ms(0));
    assert!(shake.is_animating(ms(0)) && shake.is_animating(ms(399)));
    assert!(!shake.is_animating(ms(400)), "после 0,4 с кадры не нужны");
    // Амплитуда не превышает 6 pt и убывает от колебания к колебанию (≈ 3 периода по 133 мс).
    let mut peaks = [0.0_f64; 3];
    let mut sign_changes = 0;
    let mut previous_sign = 0.0_f64;
    for millisecond in 0..400 {
        let offset = shake.offset_x_pt(ms(millisecond));
        assert!(offset.abs() <= tokens.shake_amplitude_pt + 1e-9);
        let period_index = (millisecond * 3 / 400) as usize;
        peaks[period_index] = peaks[period_index].max(offset.abs());
        if offset != 0.0 {
            if previous_sign != 0.0 && offset.signum() != previous_sign {
                sign_changes += 1;
            }
            previous_sign = offset.signum();
        }
    }
    assert!(
        peaks[0] > 0.7 * tokens.shake_amplitude_pt,
        "первое колебание почти полной амплитуды: {peaks:?}"
    );
    assert!(
        peaks[0] > peaks[1] && peaks[1] > peaks[2] && peaks[2] > 0.0,
        "затухает: {peaks:?}"
    );
    assert!(
        sign_changes >= 5,
        "три периода — не меньше пяти смен стороны: {sign_changes}"
    );
    assert_eq!(shake.offset_x_pt(ms(400)), 0.0, "в конце — на месте");
    assert_eq!(
        shake.opacity(ms(100)),
        1.0,
        "при полном движении прозрачность не трогаем"
    );
}

#[test]
fn triggering_again_restarts_the_shake_from_the_beginning() {
    let mut shake = RefusalShake::new(&tokens(), MotionPreference::Full);
    shake.trigger(ms(0));
    assert!(!shake.is_animating(ms(450)));
    shake.trigger(ms(300));
    assert!(shake.is_animating(ms(450)), "повторный запуск продлил тряску");
    let first_quarter = shake.offset_x_pt(ms(300 + 20));
    let mut fresh = RefusalShake::new(&tokens(), MotionPreference::Full);
    fresh.trigger(ms(0));
    assert_eq!(first_quarter, fresh.offset_x_pt(ms(20)), "та же кривая с нового нуля");
}

#[test]
fn reduce_motion_replaces_the_shake_with_a_dim_and_return_without_displacement() {
    let tokens = tokens();
    let mut shake = RefusalShake::new(&tokens, MotionPreference::Reduced);
    shake.trigger(ms(0));
    assert!(shake.is_animating(ms(100)));
    for millisecond in 0..300 {
        assert_eq!(shake.offset_x_pt(ms(millisecond)), 0.0, "сдвига нет");
    }
    let darkest = (0..250)
        .map(|millisecond| shake.opacity(ms(millisecond)))
        .fold(1.0_f64, f64::min);
    assert!(
        (darkest - tokens.reduced_dim_opacity).abs() < 0.01,
        "затемнение доходит до {}: {darkest}",
        tokens.reduced_dim_opacity
    );
    assert!((shake.opacity(ms(0)) - 1.0).abs() < 1e-9, "начинает с 1");
    assert_eq!(shake.opacity(ms(250)), 1.0, "возвращается к 1");
    assert!(!shake.is_animating(ms(250)), "короткое: 0,25 с");
}

#[test]
fn switching_motion_preference_applies_to_the_next_values() {
    let mut shake = RefusalShake::new(&tokens(), MotionPreference::Full);
    shake.trigger(ms(0));
    assert!(shake.offset_x_pt(ms(30)).abs() > 0.5);
    shake.set_motion(MotionPreference::Reduced);
    assert_eq!(shake.offset_x_pt(ms(30)), 0.0);
    assert!(shake.opacity(ms(60)) < 1.0);
}

// ---- токены ----

#[test]
fn attention_tokens_default_to_the_documented_numbers_and_validate() {
    let tokens = UiKitTokens::default();
    assert_eq!(tokens.attention.waiting_period_s, 1.0);
    assert_eq!(tokens.attention.shake_amplitude_pt, 6.0);
    assert_eq!(tokens.attention.shake_periods, 3.0);
    assert_eq!(tokens.attention.shake_duration_s, 0.4);
    assert!(tokens.validate().is_ok());
}

#[test]
fn bad_attention_tokens_are_rejected_by_name() {
    let mut bad = UiKitTokens::default();
    bad.attention.shake_duration_s = 0.0;
    assert!(matches!(
        bad.validate(),
        Err(TokenError::NonPositiveSize {
            name: "attention.shake_duration_s",
            ..
        })
    ));
    let mut bad = UiKitTokens::default();
    bad.attention.waiting_min_opacity = 1.0;
    assert!(matches!(
        bad.validate(),
        Err(TokenError::OutOfRange {
            name: "attention.waiting_min_opacity",
            ..
        })
    ));
}

#[test]
fn ruler_density_tokens_for_the_continuous_scales_are_two_points_per_unit() {
    use ui_kit::tokens::ScaleDensity;
    let tokens = UiKitTokens::default();
    assert_eq!(tokens.density.points_per_unit(ScaleDensity::MiredRuler), 2.0);
    assert_eq!(tokens.density.points_per_unit(ScaleDensity::TintRuler), 2.0);
    let mut bad = UiKitTokens::default();
    bad.density.mired_ruler_points_per_unit = 0.0;
    assert!(bad.validate().is_err());
}
