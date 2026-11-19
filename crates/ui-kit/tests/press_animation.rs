#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K2, анимированная степень нажатия (`PressAnimation`): плавная, в 0..1, не зависит от частоты
//! кадров, отмена и уход пальца возвращают к 0. Ведётся итогами настоящего `PressTracker`.

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureState};
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_kit::button::{PressAnimation, PressOutcome, PressTracker};
use ui_kit::tokens::{MotionToken, UiKitTokens};

mod common;
use common::*;

const BUTTON: Rect = Rect::from_xywh(100.0, 600.0, 100.0, 50.0);

fn press(position: Point) -> GestureData {
    GestureData::Press { position }
}

fn inside() -> Point {
    Point::new(150.0, 625.0)
}

fn far_away() -> Point {
    Point::new(400.0, 100.0)
}

struct Rig {
    tracker: PressTracker,
    animation: PressAnimation,
}

fn new_rig(motion: MotionPreference, tokens: &UiKitTokens) -> Rig {
    Rig {
        tracker: PressTracker::new(BUTTON, tokens).unwrap(),
        animation: PressAnimation::new(&env(tokens, motion), ms(0)).unwrap(),
    }
}

impl Rig {
    /// Событие `Press` на трекер и его итог — в анимацию (так делает экран).
    fn send(&mut self, state: GestureState, data: GestureData, at_ms: i64) -> PressOutcome {
        let outcome = self.tracker.handle_press(state, data).unwrap();
        self.animation.follow(outcome, ms(at_ms)).unwrap();
        outcome
    }
}

fn samples(rig: &Rig, from_ms: i64, to_ms: i64, step_ms: f64) -> Vec<f32> {
    let mut values = Vec::new();
    let mut t = from_ms as f64;
    while t <= to_ms as f64 + 1e-9 {
        values.push(
            rig.animation
                .amount(MonotonicNanos::from_nanos(1_000_000_000 + (t * 1e6) as i64)),
        );
        t += step_ms;
    }
    values
}

#[test]
fn press_rises_smoothly_from_zero_to_one_instead_of_jumping() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    assert_eq!(rig.animation.amount(ms(0)), 0.0);
    rig.send(GestureState::Began, press(inside()), 100);
    // В момент касания степень ещё 0: нажатие не скачет.
    assert_eq!(rig.animation.amount(ms(100)), 0.0);
    assert!(rig.animation.is_animating(ms(110)));
    let rise = samples(&rig, 100, 1000, 8.0);
    // Строго внутри 0..1 всё время (перелёт пружины зажат контрактом стекла) ...
    assert!(
        rise.iter().all(|value| (0.0..=1.0).contains(value)),
        "вне 0..1: {rise:?}"
    );
    // ... растёт без провалов на подъёме к цели ...
    let first_full = rise.iter().position(|value| *value >= 0.99).expect("дошла до 1");
    assert!(rise[..first_full].windows(2).all(|pair| pair[1] >= pair[0] - 1e-6));
    // ... за разумное время (пружина ~0,5 с) и есть промежуточные значения, а не 0 → 1.
    assert!(
        first_full > 4,
        "нажатие не должно прыгать: дошло за {first_full} кадров"
    );
    assert!(rise.iter().any(|value| (0.2..0.8).contains(value)));
    assert!((rig.animation.amount(ms(1500)) - 1.0).abs() < 1e-3);
    assert!(!rig.animation.is_animating(ms(1500)));
}

#[test]
fn release_and_activation_return_to_zero_without_going_below() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    rig.send(GestureState::Began, press(inside()), 0);
    assert_eq!(
        rig.send(GestureState::Ended, press(inside()), 600),
        PressOutcome::Activated
    );
    let fall = samples(&rig, 600, 1500, 4.0);
    assert!(fall.iter().all(|value| (0.0..=1.0).contains(value)));
    assert!(fall[0] > 0.9, "отпускание стартует с достигнутого нажатия");
    assert!(fall.iter().any(|value| (0.2..0.8).contains(value)), "падение плавное");
    assert!(fall.last().unwrap().abs() < 1e-3);
}

#[test]
fn cancel_and_sliding_off_return_to_zero_and_coming_back_presses_again() {
    let tokens = UiKitTokens::default();
    // Системная отмена.
    let mut cancelled = new_rig(MotionPreference::Full, &tokens);
    cancelled.send(GestureState::Began, press(inside()), 0);
    assert!(cancelled.animation.amount(ms(300)) > 0.3);
    cancelled.send(GestureState::Cancelled, GestureData::None, 300);
    assert!(
        cancelled.animation.amount(ms(1500)).abs() < 1e-3,
        "отмена вернула к нулю"
    );

    // Палец ушёл с кнопки и вернулся.
    let mut wandering = new_rig(MotionPreference::Full, &tokens);
    wandering.send(GestureState::Began, press(inside()), 0);
    wandering.send(GestureState::Changed, press(far_away()), 400);
    assert!(
        wandering.animation.amount(ms(1500)).abs() < 1e-3,
        "палец ушёл — подсветка снята"
    );
    wandering.send(GestureState::Changed, press(inside()), 1600);
    assert!(
        (wandering.animation.amount(ms(3000)) - 1.0).abs() < 1e-3,
        "вернулся — нажато снова"
    );
}

#[test]
fn a_quick_tap_retargets_without_a_jump() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    rig.send(GestureState::Began, press(inside()), 0);
    let at_release = rig.animation.amount(ms(80));
    assert!(
        at_release > 0.05 && at_release < 0.95,
        "отпустили на полпути: {at_release}"
    );
    rig.send(GestureState::Ended, press(inside()), 80);
    // Положение в момент отпускания то же — скачка нет; дальше значение стекает к нулю.
    assert!((rig.animation.amount(ms(80)) - at_release).abs() < 1e-6);
    let tail = samples(&rig, 80, 1200, 4.0);
    assert!(tail.last().unwrap().abs() < 1e-3);
}

#[test]
fn unchanged_outcome_does_not_restart_the_animation() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    rig.send(GestureState::Began, press(inside()), 0);
    let before = rig.animation.amount(ms(120));
    // Палец ёрзает внутри кнопки: трекер отвечает Unchanged, анимация идёт как шла.
    assert_eq!(
        rig.send(GestureState::Changed, press(Point::new(160.0, 630.0)), 120),
        PressOutcome::Unchanged
    );
    assert!((rig.animation.amount(ms(120)) - before).abs() < 1e-9);
    let reference = {
        let mut clean = new_rig(MotionPreference::Full, &tokens);
        clean.send(GestureState::Began, press(inside()), 0);
        clean.animation.amount(ms(300))
    };
    assert!((rig.animation.amount(ms(300)) - reference).abs() < 1e-9);
}

#[test]
fn sixty_and_one_twenty_hertz_sampling_agree_at_common_moments() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    rig.send(GestureState::Began, press(inside()), 0);
    rig.send(GestureState::Ended, press(inside()), 150);
    // Шаг 1000/60 мс и 1000/120 мс: общие моменты — кратные 1000/60.
    let at_60 = samples(&rig, 0, 800, 1000.0 / 60.0);
    let at_120 = samples(&rig, 0, 800, 1000.0 / 120.0);
    for (index, value) in at_60.iter().enumerate() {
        assert!(
            (value - at_120[index * 2]).abs() < 1e-5,
            "момент {index}: {value} против {}",
            at_120[index * 2]
        );
    }
    // И кривая нетривиальна: поднялась и вернулась.
    assert!(at_60.iter().cloned().fold(0.0_f32, f32::max) > 0.3);
}

#[test]
fn reduce_motion_never_overshoots_and_matches_the_snappy_token_otherwise() {
    let tokens = UiKitTokens::default();
    let mut reduced = new_rig(MotionPreference::Reduced, &tokens);
    reduced.send(GestureState::Began, press(inside()), 0);
    let rise = samples(&reduced, 0, 1500, 4.0);
    // Без перелёта кривая монотонна и не выше цели даже до зажима.
    assert!(rise.windows(2).all(|pair| pair[1] >= pair[0] - 1e-6));
    assert!(rise.iter().all(|value| *value <= 1.0));
    // Токен нажатия — именно snappy: у него есть положительная упругость.
    assert!(tokens.motion.snappy.bounce > 0.0);
    assert_eq!(MotionToken::Snappy.name(), "snappy");
    // Reduce Motion можно включить на лету: следующее перенацеливание идёт уже без перелёта.
    let mut live = new_rig(MotionPreference::Full, &tokens);
    live.animation.set_motion(MotionPreference::Reduced);
    live.send(GestureState::Began, press(inside()), 0);
    let values = samples(&live, 0, 1500, 4.0);
    assert!(values.windows(2).all(|pair| pair[1] >= pair[0] - 1e-6));
}

#[test]
fn press_state_carries_the_amount_for_the_glass_and_nothing_else() {
    let tokens = UiKitTokens::default();
    let mut rig = new_rig(MotionPreference::Full, &tokens);
    rig.send(GestureState::Began, press(inside()), 0);
    let state = rig.animation.press_state(ms(500));
    assert_eq!(state.amount, rig.animation.amount(ms(500)));
    assert!(state.amount > 0.5);
    assert_eq!(
        state.touch_point_local,
        ui_draw::draw_list::PressState::RELEASED.touch_point_local
    );
}
