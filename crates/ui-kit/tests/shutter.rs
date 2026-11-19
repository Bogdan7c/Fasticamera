#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K10 «Затвор»: нажатие через `PressTracker`, анимация без перелёта, вид от экрана, недоступный затвор.

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureState};
use ui_core::time::MotionPreference;
use ui_kit::button::PressOutcome;
use ui_kit::shutter::{ShutterButton, ShutterLook};
use ui_kit::tokens::UiKitTokens;

mod common;
use common::*;

/// Затвор Ø 80,1 с центром (220; 801,9).
const FRAME: Rect = Rect::from_xywh(179.95, 761.85, 80.1, 80.1);

fn inside() -> GestureData {
    GestureData::Press {
        position: Point::new(220.0, 801.9),
    }
}

fn outside() -> GestureData {
    GestureData::Press {
        position: Point::new(220.0, 500.0),
    }
}

fn shutter(look: ShutterLook, tokens: &UiKitTokens) -> ShutterButton {
    ShutterButton::new(FRAME, look, &env(tokens, MotionPreference::Full), ms(0)).unwrap()
}

#[test]
fn touch_down_compresses_the_disc_and_release_inside_activates_the_shutter() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Ready, &tokens);
    assert_eq!(button.snapshot(ms(0)).disc_scale, 1.0);
    let began = button.handle_press(GestureState::Began, inside(), ms(100)).unwrap();
    assert!(matches!(began, PressOutcome::StateChanged(_)));
    // Нажатие нарастает пружиной: сначала частично, потом полностью.
    let partial = button.snapshot(ms(140));
    assert!(partial.press_amount > 0.0 && partial.press_amount < 1.0);
    let full = button.snapshot(ms(1500));
    assert_eq!(full.press_amount, 1.0);
    assert!((full.disc_scale - tokens.shutter.pressed_disc_scale).abs() < 1e-9);
    // Отпускание внутри — срабатывание; диск возвращается.
    assert_eq!(
        button.handle_press(GestureState::Ended, inside(), ms(1500)).unwrap(),
        PressOutcome::Activated
    );
    assert_eq!(button.snapshot(ms(3000)).disc_scale, 1.0);
}

#[test]
fn releasing_outside_does_not_activate_and_sliding_out_releases_the_disc() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Ready, &tokens);
    button.handle_press(GestureState::Began, inside(), ms(0)).unwrap();
    let slid = button.handle_press(GestureState::Changed, outside(), ms(500)).unwrap();
    assert!(matches!(slid, PressOutcome::StateChanged(_)));
    assert!(
        button.snapshot(ms(2000)).press_amount < 1e-6,
        "палец ушёл — диск отпущен"
    );
    assert!(!matches!(
        button.handle_press(GestureState::Ended, outside(), ms(2000)).unwrap(),
        PressOutcome::Activated
    ));
}

#[test]
fn press_animation_never_overshoots_and_is_continuous_when_released_midway() {
    let tokens = UiKitTokens::default();
    for motion in [MotionPreference::Full, MotionPreference::Reduced] {
        let mut button = ShutterButton::new(FRAME, ShutterLook::Ready, &env(&tokens, motion), ms(0)).unwrap();
        button.handle_press(GestureState::Began, inside(), ms(0)).unwrap();
        let peak = (0..2000)
            .map(|t| button.snapshot(ms(t)).press_amount)
            .fold(f32::MIN, f32::max);
        assert!(peak <= 1.0 + 1e-6, "перелёта нет: {peak}");
    }
    let mut button = shutter(ShutterLook::Ready, &tokens);
    button.handle_press(GestureState::Began, inside(), ms(0)).unwrap();
    let before = button.snapshot(ms(90)).press_amount;
    button
        .handle_press(GestureState::Cancelled, GestureData::None, ms(90))
        .unwrap();
    let after = button.snapshot(ms(90)).press_amount;
    assert!(
        (before - after).abs() < 1e-6,
        "разворот без скачка: {before} -> {after}"
    );
    assert!(button.snapshot(ms(2500)).press_amount < 1e-6);
}

#[test]
fn same_time_gives_the_same_press_amount_at_60_and_120_hz() {
    let tokens = UiKitTokens::default();
    // Нажатие приходит событием со своей меткой; частота кадров в снимке не участвует вовсе.
    let mut a = shutter(ShutterLook::Ready, &tokens);
    let mut b = shutter(ShutterLook::Ready, &tokens);
    a.handle_press(GestureState::Began, inside(), ms(30)).unwrap();
    b.handle_press(GestureState::Began, inside(), ms(30)).unwrap();
    for at_ms in [46, 63, 120, 400] {
        assert_eq!(a.snapshot(ms(at_ms)), b.snapshot(ms(at_ms)));
    }
}

#[test]
fn disabled_shutter_ignores_touches_and_never_activates() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Disabled, &tokens);
    assert_eq!(
        button.handle_press(GestureState::Began, inside(), ms(0)).unwrap(),
        PressOutcome::Unchanged
    );
    assert_eq!(button.snapshot(ms(500)).press_amount, 0.0);
    assert_eq!(
        button.handle_press(GestureState::Ended, inside(), ms(600)).unwrap(),
        PressOutcome::Unchanged
    );
}

#[test]
fn becoming_disabled_mid_press_cancels_without_activation() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Ready, &tokens);
    button.handle_press(GestureState::Began, inside(), ms(0)).unwrap();
    button.set_look(ShutterLook::Disabled, ms(100)).unwrap();
    assert!(button.snapshot(ms(2500)).press_amount < 1e-6);
    assert_eq!(
        button.handle_press(GestureState::Ended, inside(), ms(300)).unwrap(),
        PressOutcome::Unchanged
    );
    // Вернули в строй — работает снова.
    button.set_look(ShutterLook::Ready, ms(400)).unwrap();
    button.handle_press(GestureState::Began, inside(), ms(500)).unwrap();
    assert_eq!(
        button.handle_press(GestureState::Ended, inside(), ms(600)).unwrap(),
        PressOutcome::Activated
    );
}

#[test]
fn pressed_look_holds_the_disc_compressed_without_a_finger() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Ready, &tokens);
    button.set_look(ShutterLook::Pressed, ms(0)).unwrap();
    let held = button.snapshot(ms(2000));
    assert_eq!(held.look, ShutterLook::Pressed);
    assert!((held.disc_scale - tokens.shutter.pressed_disc_scale).abs() < 1e-9);
    // Палец поверх вида «нажат» его не снимает: диск остаётся сжатым и после отпускания пальца.
    button.handle_press(GestureState::Began, inside(), ms(2100)).unwrap();
    button.handle_press(GestureState::Ended, inside(), ms(2200)).unwrap();
    assert!((button.snapshot(ms(4000)).disc_scale - tokens.shutter.pressed_disc_scale).abs() < 1e-9);
    button.set_look(ShutterLook::Ready, ms(4000)).unwrap();
    assert_eq!(button.snapshot(ms(6000)).disc_scale, 1.0);
}

#[test]
fn busy_shutter_still_accepts_the_press() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Busy, &tokens);
    button.handle_press(GestureState::Began, inside(), ms(0)).unwrap();
    assert_eq!(
        button.handle_press(GestureState::Ended, inside(), ms(100)).unwrap(),
        PressOutcome::Activated
    );
}

#[test]
fn wrong_gesture_data_is_an_error_and_degenerate_frame_is_rejected() {
    let tokens = UiKitTokens::default();
    let mut button = shutter(ShutterLook::Ready, &tokens);
    assert!(
        button
            .handle_press(GestureState::Began, GestureData::None, ms(0))
            .is_err()
    );
    assert!(button.set_frame(Rect::from_xywh(0.0, 0.0, 0.0, 0.0)).is_err());
    let result = ShutterButton::new(
        Rect::from_xywh(0.0, 0.0, 0.0, 0.0),
        ShutterLook::Ready,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    );
    assert!(result.is_err());
}
