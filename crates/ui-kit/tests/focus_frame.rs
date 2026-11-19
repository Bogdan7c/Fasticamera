#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K9 «Рамка фокуса»: положение — вход, появление пружиной `snappy`, удержание и затухание по
//! времени, «солнце» как встроенная шкала с пробросом событий.

use ui_core::geometry::{Point, Rect, Vector};
use ui_core::gesture::PanTranslation;
use ui_core::time::{MonotonicNanos, MotionPreference, UiDuration};
use ui_kit::drive::{DragOrigin, GrowthDirection};
use ui_kit::focus_frame::{FocusFrame, FocusFrameError, FocusFrameEvent, FocusFrameSpec, FocusHold, FocusPhase};
use ui_kit::pan::PanUpdate;
use ui_kit::scale::{ScaleEvent, TickVisibility};
use ui_kit::tokens::UiKitTokens;
use ui_kit::value::QuantizedLinear;

mod common;
use common::*;

fn auto_fade(tokens: &UiKitTokens) -> FocusFrameSpec {
    FocusFrameSpec {
        hold: FocusHold::auto_fade(&tokens.focus),
        ..FocusFrameSpec::default()
    }
}

fn frame(spec: FocusFrameSpec, motion: MotionPreference, tokens: &UiKitTokens) -> FocusFrame<QuantizedLinear> {
    FocusFrame::new(exposure_model(), 0.0, spec, &env(tokens, motion), ms(0)).unwrap()
}

type Events = Vec<FocusFrameEvent<f64>>;

fn run(frame: &mut FocusFrame<QuantizedLinear>, from_ms: i64, until_ms: i64, frame_ms: f64) -> Events {
    let mut events = Vec::new();
    let mut t = from_ms as f64;
    while t <= until_ms as f64 + 1e-9 {
        frame
            .advance(MonotonicNanos::from_nanos(1_000_000_000 + (t * 1e6) as i64))
            .unwrap();
        events.extend(frame.drain_events());
        t += frame_ms;
    }
    events
}

fn vertical(dy: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(0.0, dy),
        from_recognition: Vector::new(0.0, dy),
    }
}

/// Тянет палец вверх на `up_pt` от момента `start_ms` с шагом 8 мс и отпускает.
fn drag_sun_up(frame: &mut FocusFrame<QuantizedLinear>, start_ms: i64, up_pt: f64) {
    frame
        .apply_pan(
            ms(start_ms),
            PanUpdate::Began {
                translation: vertical(0.0),
            },
        )
        .unwrap();
    for step in 1..=10 {
        let dy = -up_pt * f64::from(step) / 10.0;
        frame
            .apply_pan(
                ms(start_ms + i64::from(step) * 8),
                PanUpdate::Changed {
                    translation: vertical(dy),
                },
            )
            .unwrap();
    }
    frame
        .apply_pan(ms(start_ms + 88), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
}

#[test]
fn hidden_until_placed_then_appears_at_the_given_point_from_a_larger_scale() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    assert_eq!(frame.phase(), FocusPhase::Hidden);
    assert_eq!(frame.snapshot(ms(0)).opacity, 0.0);

    frame.place(Point::new(200.0, 400.0), ms(100)).unwrap();
    let first = frame.snapshot(ms(100));
    assert_eq!(
        (first.phase, first.center),
        (FocusPhase::Active, Point::new(200.0, 400.0))
    );
    assert!((first.scale - tokens.focus.appear_scale).abs() < 1e-9);
    // Через мгновение масштаб уже падает к 1, а рамка проявляется.
    let later = frame.snapshot(ms(150));
    assert!(later.scale < first.scale && later.scale > 1.0 - 0.2);
    assert!(later.opacity > 0.0);
    // В покое — ровно 1 и непрозрачна.
    let settled = frame.snapshot(ms(1500));
    assert_eq!((settled.scale, settled.opacity), (1.0, 1.0));
}

#[test]
fn reduce_motion_removes_the_snappy_undershoot_of_the_scale() {
    let tokens = UiKitTokens::default();
    let lowest_scale = |motion| {
        let mut frame = frame(auto_fade(&tokens), motion, &tokens);
        frame.place(Point::new(100.0, 100.0), ms(0)).unwrap();
        (0..1500).map(|t| frame.snapshot(ms(t)).scale).fold(f64::MAX, f64::min)
    };
    assert!(lowest_scale(MotionPreference::Full) < 0.999, "snappy перелетает за 1");
    assert!(lowest_scale(MotionPreference::Reduced) >= 1.0 - 1e-9);
}

#[test]
fn auto_fade_dims_then_hides_and_reports_each_step_exactly_once() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    // До срока потускнения рамка в полную силу.
    let events = run(&mut frame, 0, 2900, 16.0);
    assert!(events.is_empty());
    assert_eq!(frame.snapshot(ms(2900)).opacity, 1.0);
    // Срок потускнения 3 с: событие одно, прозрачность оседает на токене.
    let events = run(&mut frame, 2900, 4500, 16.0);
    assert_eq!(events, vec![FocusFrameEvent::Dimmed]);
    assert_eq!(frame.phase(), FocusPhase::Dimmed);
    assert!((f64::from(frame.snapshot(ms(4500)).opacity) - tokens.focus.dimmed_opacity).abs() < 1e-4);
    // Срок исчезновения 5 с: рамка уходит и сообщает один раз.
    let events = run(&mut frame, 4500, 8000, 16.0);
    assert_eq!(events, vec![FocusFrameEvent::Hidden]);
    let end = frame.snapshot(ms(8000));
    assert_eq!((end.phase, end.opacity), (FocusPhase::Hidden, 0.0));
}

#[test]
fn hold_until_released_keeps_the_frame_at_full_strength() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    let events = run(&mut frame, 0, 120_000, 100.0);
    assert!(events.is_empty());
    let snapshot = frame.snapshot(ms(120_000));
    assert_eq!((snapshot.phase, snapshot.opacity), (FocusPhase::Active, 1.0));
}

#[test]
fn locking_a_dimmed_frame_restores_it_and_unlocking_restarts_the_timer() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    run(&mut frame, 0, 3500, 16.0);
    assert_eq!(frame.phase(), FocusPhase::Dimmed);
    // «Держать, пока не скажу»: рамка возвращается в полную силу и больше не гаснет.
    frame.set_hold(FocusHold::UntilReleased, ms(3500)).unwrap();
    run(&mut frame, 3500, 30_000, 100.0);
    assert_eq!(
        (frame.phase(), frame.snapshot(ms(30_000)).opacity),
        (FocusPhase::Active, 1.0)
    );
    // Снятие блокировки: срок отсчитывается заново от этого момента (3 с до потускнения).
    frame.set_hold(FocusHold::auto_fade(&tokens.focus), ms(30_000)).unwrap();
    run(&mut frame, 30_000, 32_900, 16.0);
    assert_eq!(frame.phase(), FocusPhase::Active);
    run(&mut frame, 32_900, 33_200, 16.0);
    assert_eq!(frame.phase(), FocusPhase::Dimmed);
}

#[test]
fn fading_timeline_does_not_depend_on_the_frame_rate() {
    let tokens = UiKitTokens::default();
    let mut slow = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    let mut fast = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    slow.place(Point::new(10.0, 10.0), ms(0)).unwrap();
    fast.place(Point::new(10.0, 10.0), ms(0)).unwrap();
    let slow_events = run(&mut slow, 0, 4000, 1000.0 / 60.0);
    let fast_events = run(&mut fast, 0, 4000, 1000.0 / 120.0);
    assert_eq!(slow_events, fast_events);
    for at_ms in [3000, 3100, 3500, 4000] {
        assert_eq!(slow.snapshot(ms(at_ms)), fast.snapshot(ms(at_ms)), "на {at_ms} мс");
    }
}

#[test]
fn touching_the_sun_counts_as_activity_and_postpones_the_fade() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(auto_fade(&tokens), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    run(&mut frame, 0, 2500, 16.0);
    drag_sun_up(&mut frame, 2500, 40.0);
    // Исходный срок потускнения (3 с) прошёл, но активность была в 2,5–2,6 с: рамка ещё в силе.
    run(&mut frame, 2600, 3500, 16.0);
    assert_eq!(frame.phase(), FocusPhase::Active);
    run(&mut frame, 3500, 5700, 16.0);
    assert_eq!(frame.phase(), FocusPhase::Dimmed);
}

#[test]
fn sun_drag_changes_exposure_forwards_scale_events_and_moves_the_sun_up() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    drag_sun_up(&mut frame, 100, 40.0);
    let events = frame.drain_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, FocusFrameEvent::Sun(ScaleEvent::ValueChanged { value }) if *value > 0.0)),
        "события шкалы пробрасываются: {events:?}"
    );
    run(&mut frame, 200, 1500, 16.0);
    let snapshot = frame.snapshot(ms(1500));
    assert!(snapshot.sun.value > 0.0, "палец вверх увеличивает экспозицию");
    assert!(snapshot.sun_travel_up_pt > 0.0, "солнце поднялось");
    // Новая точка фокуса берёт текущее положение за исходное: солнце снова у рамки, значение сохранено.
    let value = snapshot.sun.value;
    frame.place(Point::new(50.0, 60.0), ms(1600)).unwrap();
    let after = frame.snapshot(ms(1600));
    assert_eq!(after.sun_travel_up_pt, 0.0);
    assert_eq!(after.sun.value, value);
}

#[test]
fn programmatic_exposure_reset_is_silent_for_the_finger_haptics() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    drag_sun_up(&mut frame, 100, 40.0);
    run(&mut frame, 200, 1500, 16.0);
    frame.drain_events();
    frame
        .set_sun_value(0.0, ui_kit::drive::ScrollTransition::Immediate, ms(1600))
        .unwrap();
    frame.advance(ms(1616)).unwrap();
    let events = frame.drain_events();
    assert_eq!(frame.snapshot(ms(1616)).sun.value, 0.0);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, FocusFrameEvent::Sun(ScaleEvent::Haptic { .. })))
    );
}

#[test]
fn dismiss_fades_the_frame_out_and_reports_hidden_once() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    run(&mut frame, 0, 1000, 16.0);
    frame.dismiss(ms(1000)).unwrap();
    assert_eq!(frame.phase(), FocusPhase::Fading);
    assert_eq!(run(&mut frame, 1000, 3000, 16.0), vec![FocusFrameEvent::Hidden]);
    // Повторное убирание скрытой рамки — no-op без событий.
    frame.dismiss(ms(3000)).unwrap();
    assert!(run(&mut frame, 3000, 3500, 16.0).is_empty());
}

#[test]
fn node_frame_uses_the_measured_size_around_the_point() {
    let tokens = UiKitTokens::default();
    let frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    let rect = frame.node_frame(Point::new(200.0, 400.0));
    assert_eq!(rect, Rect::from_xywh(200.0 - 45.5, 400.0 - 46.0, 91.0, 92.0));
}

#[test]
fn description_and_calls_are_validated_instead_of_fixed_silently() {
    let tokens = UiKitTokens::default();
    let create = |spec| {
        FocusFrame::new(
            exposure_model(),
            0.0,
            spec,
            &env(&tokens, MotionPreference::Full),
            ms(0),
        )
    };
    let horizontal = FocusFrameSpec {
        sun: ui_kit::scale::ScaleSpec {
            direction: GrowthDirection::FingerRight,
            ticks: TickVisibility::Hidden,
            drag_origin: DragOrigin::FromRecognition,
            ..FocusFrameSpec::default().sun
        },
        ..FocusFrameSpec::default()
    };
    assert!(matches!(create(horizontal), Err(FocusFrameError::SunMustBeVertical(_))));
    let with_ticks = FocusFrameSpec {
        sun: ui_kit::scale::ScaleSpec {
            ticks: TickVisibility::Visible,
            ..FocusFrameSpec::default().sun
        },
        ..FocusFrameSpec::default()
    };
    assert!(matches!(create(with_ticks), Err(FocusFrameError::SunTicksMustBeHidden)));
    let bad_hold = FocusFrameSpec {
        hold: FocusHold::AutoFade {
            dim_after: UiDuration::from_millis(5000),
            hide_after: UiDuration::from_millis(3000),
        },
        ..FocusFrameSpec::default()
    };
    assert!(matches!(create(bad_hold), Err(FocusFrameError::InvalidHold)));

    let mut hidden = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    let pan = hidden.apply_pan(
        ms(0),
        PanUpdate::Began {
            translation: vertical(0.0),
        },
    );
    assert_eq!(pan, Err(FocusFrameError::NotVisible));
    assert!(matches!(
        hidden.place(Point::new(f64::NAN, 0.0), ms(0)),
        Err(FocusFrameError::InvalidPoint(_))
    ));
    assert_eq!(
        hidden.phase(),
        FocusPhase::Hidden,
        "неудачный place состояния не меняет"
    );
}

fn touch_sun(frame: &mut FocusFrame<QuantizedLinear>, at_ms: i64) -> Result<(), FocusFrameError> {
    frame.apply_pan(
        ms(at_ms),
        PanUpdate::Began {
            translation: vertical(0.0),
        },
    )
}

#[test]
fn touching_the_sun_of_a_fading_frame_brings_the_frame_back() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    run(&mut frame, 0, 1000, 16.0);
    frame.dismiss(ms(1000)).unwrap();
    assert_eq!(frame.phase(), FocusPhase::Fading);
    // Палец лёг на «солнце», пока рамка гаснет: рамка возвращается, `Hidden` не выдаётся.
    touch_sun(&mut frame, 1050).unwrap();
    assert_eq!(frame.phase(), FocusPhase::Active);
    let events = run(&mut frame, 1050, 4000, 16.0);
    assert!(!events.contains(&FocusFrameEvent::Hidden), "{events:?}");
    let snapshot = frame.snapshot(ms(4000));
    assert_eq!((snapshot.phase, snapshot.opacity), (FocusPhase::Active, 1.0));
}

#[test]
fn touching_the_sun_after_the_fade_finished_is_rejected_and_hidden_is_reported() {
    let tokens = UiKitTokens::default();
    let mut frame = frame(FocusFrameSpec::default(), MotionPreference::Full, &tokens);
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    run(&mut frame, 0, 1000, 16.0);
    frame.dismiss(ms(1000)).unwrap();
    // Исчезновение закончилось, но кадр его ещё не заметил: рамки уже нет, жест отвергается.
    let result = touch_sun(&mut frame, 4000);
    assert!(matches!(result, Err(FocusFrameError::NotVisible)), "{result:?}");
    assert_eq!(frame.phase(), FocusPhase::Hidden);
    assert_eq!(frame.drain_events(), vec![FocusFrameEvent::Hidden]);
}
