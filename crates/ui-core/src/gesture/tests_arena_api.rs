//! Тесты публичного API арены: свайп, отношения, конфиги, правило пальцев, входные ошибки.

use shell_contract::touch::TouchPhase;

use super::test_support::*;
use super::*;
use crate::geometry::{Point, Vector};
use crate::velocity::VelocityConfig;

const VIEW: u32 = 1;

fn expect_swipe(event: &GestureEvent<&'static str>) -> (SwipeDirection, Point, Vector) {
    match event.data {
        GestureData::Swipe {
            direction,
            position,
            velocity,
        } => (direction, position, velocity),
        other => panic!("ожидали свайп: {other:?}"),
    }
}

// ------------------------------------------------------------------ свайп

#[test]
fn swipe_is_recognized_mid_motion_before_the_finger_lifts() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(
        node(VIEW),
        swipe(&[SwipeDirection::Left, SwipeDirection::Right]),
        "swipe",
    );
    rig.down(0, 200.0, 100.0, 0, &path);
    // 1250 pt/с влево: на 40 pt (> 30) решение принимается, отрыва ещё нет.
    for step in 1..=5 {
        rig.moved(0, 200.0 - 10.0 * f64::from(step), 100.0, 8 * i64::from(step));
    }
    let events = rig.events();
    assert_eq!(events.len(), 1, "решение принято один раз, по ходу движения");
    assert_eq!(events[0].state, GestureState::Ended);
    assert_eq!(events[0].time, at(24), "на третьем сдвиге: дистанция 30 pt набрана");
    let (direction, _, velocity) = expect_swipe(&events[0]);
    assert_eq!(direction, SwipeDirection::Left);
    assert!((velocity.dx + 1250.0).abs() < 1.0, "{velocity:?}");
    rig.up(0, 150.0, 100.0, 60);
    assert!(
        rig.trace().is_empty(),
        "отрыв после распознанного свайпа ничего не добавляет"
    );
    assert!(rig.arena.is_idle());
}

#[test]
fn swipe_in_a_forbidden_direction_fails() {
    let mut rig = Rig::new();
    rig.add(
        node(VIEW),
        swipe(&[SwipeDirection::Left, SwipeDirection::Right]),
        "swipe",
    );
    rig.down(0, 100.0, 300.0, 0, &[node(VIEW)]);
    rig.moved(0, 100.0, 280.0, 10);
    rig.moved(0, 100.0, 265.0, 20); // вверх на 35 pt: направления «вверх» нет в наборе
    assert_eq!(rig.trace(), ["swipe:Failed"]);
}

#[test]
fn slow_swipe_fails_by_velocity_and_by_duration_on_frames() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), swipe(&[SwipeDirection::Right]), "swipe");
    // 40 pt за 300 мс = 133 pt/с < 200: дистанция есть, скорости нет — пока ждём.
    rig.down(0, 0.0, 0.0, 0, &path);
    for step in 1..=6 {
        rig.moved(0, 40.0 * f64::from(step) / 6.0, 0.0, 50 * i64::from(step));
    }
    assert!(rig.trace().is_empty());
    // Палец замер, идут кадры: после 400 мс свайп проваливается по времени.
    rig.frame(400);
    assert!(rig.trace().is_empty());
    rig.frame(401);
    assert_eq!(rig.trace(), ["swipe:Failed"]);
}

#[test]
fn swipe_that_is_fast_enough_but_starts_late_fails_by_duration() {
    let mut rig = Rig::new();
    rig.add(node(VIEW), swipe(&[SwipeDirection::Right]), "swipe");
    rig.down(0, 0.0, 0.0, 0, &[node(VIEW)]);
    rig.moved(0, 5.0, 0.0, 100);
    rig.moved(0, 80.0, 0.0, 450); // быстрый рывок, но позже 400 мс
    assert_eq!(rig.trace(), ["swipe:Failed"]);
}

#[test]
fn pan_that_requires_swipe_to_fail_waits_then_starts_after_swipe_timeout() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    let swipe_id = rig.add(node(VIEW), swipe(&[SwipeDirection::Right]), "swipe");
    let pan_id = rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.arena.require_to_fail(pan_id, swipe_id).expect("верно");
    // Медленное перетаскивание: пан готов на 11 pt, но ждёт провала свайпа.
    rig.down(0, 0.0, 0.0, 0, &path);
    rig.moved(0, 11.0, 0.0, 100);
    rig.moved(0, 20.0, 0.0, 200);
    assert!(rig.trace().is_empty());
    assert_eq!(rig.state(pan_id), GestureState::Possible);
    rig.frame(401);
    assert_eq!(rig.trace(), ["swipe:Failed", "pan:Began"]);

    // Быстрый рывок: свайп побеждает, пан проваливается без Began.
    rig.up(0, 20.0, 0.0, 450);
    rig.trace();
    rig.down(0, 0.0, 0.0, 1000, &path);
    rig.moved(0, 12.0, 0.0, 1008);
    rig.moved(0, 24.0, 0.0, 1016);
    rig.moved(0, 36.0, 0.0, 1024);
    assert_eq!(rig.trace(), ["swipe:Ended", "pan:Failed"]);
}

// ------------------------------------------------------------------ отношения

#[test]
fn relation_errors_are_explicit() {
    let mut rig = Rig::new();
    let first = rig.add(node(VIEW), tap(), "first");
    let second = rig.add(node(VIEW), tap(), "second");
    let third = rig.add(node(VIEW), tap(), "third");
    assert_eq!(
        rig.arena.require_to_fail(first, first),
        Err(GestureError::Relation(RelationError::WithItself(first)))
    );
    assert_eq!(
        rig.arena.allow_simultaneous(second, second),
        Err(GestureError::Relation(RelationError::WithItself(second)))
    );
    rig.arena.require_to_fail(first, second).expect("верно");
    rig.arena
        .require_to_fail(first, second)
        .expect("повтор той же пары — не ошибка");
    rig.arena.require_to_fail(second, third).expect("верно");
    assert_eq!(
        rig.arena.require_to_fail(second, first),
        Err(GestureError::Relation(RelationError::Cycle {
            waiting: second,
            must_fail: first
        }))
    );
    assert_eq!(
        rig.arena.require_to_fail(third, first),
        Err(GestureError::Relation(RelationError::Cycle {
            waiting: third,
            must_fail: first
        })),
        "косвенный цикл тоже ловится"
    );
    rig.arena.remove_recognizer(third).expect("известен");
    assert_eq!(
        rig.arena.require_to_fail(first, third),
        Err(GestureError::UnknownRecognizer(third))
    );
    assert_eq!(
        rig.arena.allow_simultaneous(third, first),
        Err(GestureError::UnknownRecognizer(third))
    );
    assert_eq!(
        rig.arena.remove_recognizer(third),
        Err(GestureError::UnknownRecognizer(third))
    );
    // Цикл через удалённого уже невозможен: отношения с ним забыты.
    rig.arena
        .require_to_fail(second, first)
        .expect_err("цикл first→second остаётся");
}

#[test]
fn simultaneous_pair_both_win_and_default_pair_does_not() {
    let path = [node(VIEW)];
    // Пан (игнорирует лишние пальцы) + щипок: по умолчанию щипок проваливается при начале пана.
    let build = |allow: bool| {
        let mut rig = Rig::new();
        let pan_id = rig.add(
            node(VIEW),
            RecognizerSpec::Pan(PanSpec {
                extra_fingers: ExtraFingers::Ignore,
                ..PanSpec::default()
            }),
            "pan",
        );
        let pinch_id = rig.add(node(VIEW), pinch(), "pinch");
        if allow {
            rig.arena.allow_simultaneous(pan_id, pinch_id).expect("известны");
        }
        rig.down(0, 100.0, 100.0, 0, &path);
        rig.moved(0, 120.0, 100.0, 16);
        rig
    };
    let mut exclusive = build(false);
    assert_eq!(exclusive.trace(), ["pan:Began", "pinch:Failed"]);

    let mut together = build(true);
    assert_eq!(together.trace(), ["pan:Began"]);
    together.down(1, 220.0, 100.0, 32, &path); // расстояние 100
    together.moved(1, 260.0, 100.0, 48); // 140: щипок начинается
    together.moved(0, 125.0, 100.0, 64);
    assert_eq!(
        together.trace(),
        ["pinch:Began", "pan:Changed", "pinch:Changed"].map(String::from),
        "пан не отменён вторым пальцем"
    );
}

#[test]
fn extra_fingers_ignore_keeps_the_gesture_on_the_first_finger() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(
        node(VIEW),
        RecognizerSpec::Pan(PanSpec {
            extra_fingers: ExtraFingers::Ignore,
            ..PanSpec::default()
        }),
        "pan",
    );
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.moved(0, 130.0, 100.0, 16);
    rig.down(1, 300.0, 300.0, 20, &path);
    rig.moved(1, 400.0, 400.0, 30);
    let events = rig.events();
    assert_eq!(
        events.iter().map(|event| event.state).collect::<Vec<_>>(),
        [GestureState::Began]
    );
    rig.moved(0, 150.0, 100.0, 40);
    let changed = rig.events();
    assert!(matches!(changed[0].data, GestureData::Pan { position, .. } if position == Point::new(150.0, 100.0)));
    // Отрыв чужого пальца ничего не меняет, отрыв своего — Ended.
    rig.up(1, 400.0, 400.0, 50);
    assert!(rig.trace().is_empty());
    rig.up(0, 150.0, 100.0, 60);
    assert_eq!(rig.trace(), ["pan:Ended"]);
}

// ------------------------------------------------------------------ конфиги

#[test]
fn invalid_specs_are_rejected_and_do_not_consume_ids() {
    let mut rig = Rig::new();
    let bad_specs = [
        RecognizerSpec::Tap(TapSpec {
            max_movement_pt: 0.0,
            ..TapSpec::default()
        }),
        RecognizerSpec::Tap(TapSpec {
            max_distance_between_taps_pt: f64::NAN,
            ..TapSpec::default()
        }),
        RecognizerSpec::Tap(TapSpec {
            max_interval_between_taps_ms: 0,
            ..TapSpec::default()
        }),
        RecognizerSpec::LongPress(LongPressSpec {
            min_duration_ms: 0,
            ..LongPressSpec::default()
        }),
        RecognizerSpec::LongPress(LongPressSpec {
            allowable_movement_pt: -1.0,
            ..LongPressSpec::default()
        }),
        RecognizerSpec::Pan(PanSpec {
            hysteresis_pt: 0.0,
            ..PanSpec::default()
        }),
        RecognizerSpec::Pinch(PinchSpec {
            start_distance_change_pt: f64::INFINITY,
        }),
        RecognizerSpec::Swipe(SwipeSpec {
            directions: vec![],
            ..SwipeSpec::default()
        }),
        RecognizerSpec::Swipe(SwipeSpec {
            min_distance_pt: 0.0,
            ..SwipeSpec::default()
        }),
        RecognizerSpec::Swipe(SwipeSpec {
            max_duration_ms: 0,
            ..SwipeSpec::default()
        }),
        RecognizerSpec::Swipe(SwipeSpec {
            min_velocity_pt_per_s: -5.0,
            ..SwipeSpec::default()
        }),
    ];
    for spec in bad_specs {
        let result = rig.arena.add_recognizer(node(VIEW), spec.clone(), "bad");
        assert!(
            matches!(result, Err(GestureError::InvalidSpec(_))),
            "{spec:?} → {result:?}"
        );
    }
    assert_eq!(
        rig.arena.add_recognizer(
            node(VIEW),
            RecognizerSpec::Swipe(SwipeSpec {
                directions: vec![],
                ..SwipeSpec::default()
            }),
            "x"
        ),
        Err(GestureError::InvalidSpec(SpecError::EmptySwipeDirections))
    );
    let good = rig.add(node(VIEW), press(), "good");
    assert_eq!(good.ordinal(), 0, "неудачные добавления номеров не тратят");
    for spec in [
        tap(),
        double_tap(),
        long_press(),
        pan(PanDirection::Any),
        pinch(),
        press(),
        swipe(&[SwipeDirection::Up]),
    ] {
        assert_eq!(spec.validate(), Ok(()));
    }
}

#[test]
fn arena_config_validates_velocity_and_parses_from_json() {
    let broken = GestureConfig {
        velocity: VelocityConfig {
            horizon_ms: 0,
            ..VelocityConfig::default()
        },
    };
    assert!(matches!(
        GestureArena::<&str>::new(broken),
        Err(GestureError::InvalidVelocityConfig(_))
    ));
    let parsed: GestureConfig = serde_json::from_str(r#"{"velocity":{"horizon_ms":80}}"#).expect("разбор");
    assert_eq!(parsed.velocity.horizon_ms, 80);
    assert!(serde_json::from_str::<GestureConfig>(r#"{"unknown":1}"#).is_err());
    let spec: RecognizerSpec = serde_json::from_str(r#"{"pan":{"direction":"left"}}"#).expect("разбор спецификации");
    assert_eq!(
        spec,
        RecognizerSpec::Pan(PanSpec {
            direction: PanDirection::Left,
            ..PanSpec::default()
        })
    );
}

#[test]
fn pan_direction_accepts_by_dominant_axis_and_sign() {
    let right = Vector::new(12.0, 3.0);
    let left = Vector::new(-12.0, 3.0);
    let up = Vector::new(3.0, -12.0);
    let down = Vector::new(3.0, 12.0);
    let diagonal_tie = Vector::new(10.0, -10.0);
    let expectations = [
        (PanDirection::Any, [true, true, true, true]),
        (PanDirection::Horizontal, [true, true, false, false]),
        (PanDirection::Vertical, [false, false, true, true]),
        (PanDirection::Right, [true, false, false, false]),
        (PanDirection::Left, [false, true, false, false]),
        (PanDirection::Up, [false, false, true, false]),
        (PanDirection::Down, [false, false, false, true]),
    ];
    for (direction, expected) in expectations {
        let actual = [right, left, up, down].map(|vector| direction.accepts(vector));
        assert_eq!(actual, expected, "{direction:?}");
    }
    assert!(PanDirection::Horizontal.accepts(diagonal_tie), "ничья — горизонталь");
    assert!(!PanDirection::Vertical.accepts(diagonal_tie));
}

// ------------------------------------------------------------------ вход и инварианты

#[test]
fn invalid_input_is_reported_and_changes_nothing() {
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "tap");
    let path = [node(VIEW)];
    assert_eq!(
        rig.touch(0, TouchPhase::Began, f64::NAN, 1.0, 0, &path),
        PointerDisposition::IgnoredInvalidPosition
    );
    assert!(rig.arena.is_idle(), "NaN-касание не открыло последовательность");
    assert_eq!(
        rig.touch(5, TouchPhase::Ended, 1.0, 1.0, 10, &[]),
        PointerDisposition::IgnoredUnknownPointer
    );
    rig.down(0, 1.0, 1.0, 20, &path);
    assert_eq!(
        rig.touch(0, TouchPhase::Began, 2.0, 2.0, 21, &path),
        PointerDisposition::IgnoredDuplicateBegan
    );
    rig.up(0, 1.0, 1.0, 40);
    assert_eq!(rig.trace(), ["tap:Ended"], "дубликат не сломал палец");
}

#[test]
fn frame_time_results_distinguish_advance_repeat_and_backwards() {
    let mut rig = Rig::new();
    assert!(matches!(
        rig.arena.advance_time(at(10)),
        crate::time::ClockAdvance::Advanced { .. }
    ));
    assert_eq!(rig.arena.advance_time(at(10)), crate::time::ClockAdvance::Repeated);
    assert!(matches!(
        rig.arena.advance_time(at(5)),
        crate::time::ClockAdvance::IgnoredBackwards { .. }
    ));
}

#[test]
fn touch_stamped_before_an_earlier_frame_is_still_processed() {
    // Кадр на 600 мс уже пришёл, а касание с меткой 580 мс догоняет его: его нельзя отбрасывать.
    let mut rig = Rig::new();
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.down(0, 0.0, 0.0, 0, &[node(VIEW)]);
    rig.frame(600);
    rig.moved(0, 20.0, 0.0, 580);
    assert_eq!(rig.trace(), ["pan:Began"]);
}

#[test]
fn recognizers_added_mid_touch_join_only_the_next_sequence() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "old");
    rig.down(0, 1.0, 1.0, 0, &path);
    let late = rig.add(node(VIEW), tap(), "late");
    rig.up(0, 1.0, 1.0, 30);
    assert_eq!(rig.trace(), ["old:Ended"]);
    assert_eq!(rig.state(late), GestureState::Possible);
    rig.down(0, 1.0, 1.0, 100, &path);
    rig.up(0, 1.0, 1.0, 130);
    assert_eq!(rig.trace(), ["old:Ended", "late:Failed"]);
}

#[test]
fn arena_never_hangs_in_possible_after_every_scenario_finishes() {
    // После любого завершённого касания и бегущих кадров арена должна прийти в покой.
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    let single = rig.add(node(VIEW), tap(), "single");
    let double = rig.add(node(VIEW), double_tap(), "double");
    rig.arena.require_to_fail(single, double).expect("верно");
    rig.add(node(VIEW), long_press(), "long");
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.add(node(VIEW), pinch(), "pinch");
    rig.add(node(VIEW), swipe(&[SwipeDirection::Left]), "swipe");
    rig.add(node(VIEW), press(), "press");
    // Тап, затем долгое ничего, затем пан, затем два пальца.
    rig.down(0, 50.0, 50.0, 0, &path);
    rig.up(0, 50.0, 50.0, 40);
    rig.frame(400);
    assert!(rig.arena.is_idle());
    rig.down(0, 50.0, 50.0, 1000, &path);
    rig.moved(0, 90.0, 50.0, 1050);
    rig.up(0, 90.0, 50.0, 1100);
    rig.frame(1500);
    assert!(rig.arena.is_idle());
    rig.down(0, 50.0, 50.0, 2000, &path);
    rig.down(1, 150.0, 50.0, 2010, &path);
    rig.up(0, 50.0, 50.0, 2100);
    rig.up(1, 150.0, 50.0, 2110);
    rig.frame(2600);
    assert!(rig.arena.is_idle());
    for id in 0..7 {
        let known = rig.arena.recognizer_state(RecognizerId(id)).expect("известен");
        assert_eq!(known, GestureState::Possible, "распознаватель {id} сброшен");
    }
}
