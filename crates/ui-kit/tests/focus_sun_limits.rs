#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K9: `FocusFrame::set_sun_limits` — смена пределов «солнца» без пересоздания рамки: значение
//! сжимается в новые пределы без хаптики, рамка (положение, фаза) остаётся как была, ошибка не портит.

use ui_core::geometry::Point;
use ui_core::time::MotionPreference;
use ui_kit::drive::ScrollTransition;
use ui_kit::focus_frame::{FocusFrame, FocusFrameError, FocusFrameEvent, FocusFrameSpec, FocusPhase};
use ui_kit::scale::ScaleEvent;
use ui_kit::tokens::UiKitTokens;
use ui_kit::value::{LinearLimits, QuantizedLinear, ValueModel};

mod common;
use common::*;

fn frame(initial_ev: f64, tokens: &UiKitTokens) -> FocusFrame<QuantizedLinear> {
    FocusFrame::new(
        exposure_model(),
        initial_ev,
        FocusFrameSpec::default(),
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

const NARROW: LinearLimits = LinearLimits { min: -1.0, max: 1.0 };

#[test]
fn value_above_new_limits_is_clamped_immediately_without_haptics() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(5.0 / 3.0, &tokens);
    assert!((frame.sun().value(ms(0)) - 5.0 / 3.0).abs() < 1e-9);
    frame.place(Point::new(200.0, 300.0), ms(10)).unwrap();
    frame.drain_events();

    frame
        .set_sun_limits(NARROW, ScrollTransition::Immediate, ms(20))
        .unwrap();
    // Значение сразу в новых пределах, пределы модели сменились.
    assert!((frame.sun().value(ms(20)) - 1.0).abs() < 1e-9);
    assert_eq!(*frame.sun().model().limits(), NARROW);
    let events = frame.drain_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, FocusFrameEvent::Sun(ScaleEvent::ValueChanged { value }) if (*value - 1.0).abs() < 1e-9)),
        "ожидали ValueChanged до 1,0: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FocusFrameEvent::Sun(ScaleEvent::Haptic { .. }))),
        "смена пределов не вибрирует: {events:?}"
    );
}

#[test]
fn animated_transition_settles_inside_new_limits_and_keeps_the_frame_untouched() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(-2.0, &tokens);
    frame.place(Point::new(200.0, 300.0), ms(10)).unwrap();
    let phase_before = frame.phase();
    frame
        .set_sun_limits(NARROW, ScrollTransition::Animated, ms(20))
        .unwrap();
    // Рамка не тронута: фаза и положение прежние.
    assert_eq!(frame.phase(), phase_before);
    assert_eq!(frame.phase(), FocusPhase::Active);
    assert_eq!(frame.snapshot(ms(20)).center, Point::new(200.0, 300.0));
    // Значение доезжает до нижней границы -1 и не выходит за неё.
    let mut last = 0.0;
    for step in 0..120 {
        let now = ms(20 + step * 16);
        frame.advance(now).unwrap();
        last = frame.sun().value(now);
    }
    assert!((last + 1.0).abs() < 1e-9, "значение должно осесть на -1, а не {last}");
}

#[test]
fn value_inside_new_limits_does_not_move_and_widening_keeps_the_value() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(0.0, &tokens);
    frame.place(Point::new(200.0, 300.0), ms(10)).unwrap();
    frame.drain_events();
    let offset_before = frame.sun().snapshot(ms(20)).offset_points;
    frame
        .set_sun_limits(NARROW, ScrollTransition::Immediate, ms(20))
        .unwrap();
    assert_eq!(frame.sun().value(ms(20)), 0.0);
    assert_eq!(frame.sun().snapshot(ms(20)).offset_points, offset_before);
    // Расширение пределов обратно тоже не двигает значение.
    frame
        .set_sun_limits(
            LinearLimits { min: -3.0, max: 3.0 },
            ScrollTransition::Immediate,
            ms(30),
        )
        .unwrap();
    assert_eq!(frame.sun().value(ms(30)), 0.0);
    assert!(
        frame
            .drain_events()
            .iter()
            .all(|event| !matches!(event, FocusFrameEvent::Sun(ScaleEvent::Haptic { .. })))
    );
}

#[test]
fn invalid_limits_are_an_error_and_leave_the_sun_unchanged() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(0.5, &tokens);
    let result = frame.set_sun_limits(
        LinearLimits { min: 2.0, max: -2.0 },
        ScrollTransition::Immediate,
        ms(20),
    );
    assert!(matches!(result, Err(FocusFrameError::Scale(_))), "{result:?}");
    assert_eq!(*frame.sun().model().limits(), LinearLimits { min: -2.0, max: 2.0 });
    assert!((frame.sun().value(ms(20)) - 0.5).abs() < 0.17);
}
