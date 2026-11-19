#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! «Ведение за пальцем» шкалы — главное ощущение iOS: 1:1, без скачка на гистерезис, без
//! ступеньки скорости при отпускании, касание останавливает движение, за краем — резинка.

use ui_core::time::MotionPreference;
use ui_kit::drive::{DragOrigin, GrowthDirection, MotionPhase};
use ui_kit::scale::ScaleEvent;
use ui_kit::tokens::UiKitTokens;

mod common;
use common::*;

const STEP_POINTS: f64 = 12.0;

#[test]
fn content_follows_the_finger_one_to_one_on_every_frame_in_both_directions() {
    let tokens = UiKitTokens::default();
    for (direction, sign) in [(GrowthDirection::FingerRight, 1.0), (GrowthDirection::FingerLeft, -1.0)] {
        let mut scale = exposure_scale(0.0, exposure_spec(direction, DragOrigin::FromRecognition), &tokens);
        let start = scale.snapshot(ms(0)).offset_points;
        scale.drag_began(ms(0), pan_same(0.0)).unwrap();
        for frame in 1..=24 {
            let finger_dx = sign * 2.0 * frame as f64;
            let now = ms(frame * 16);
            scale.drag_moved(now, pan_same(finger_dx)).unwrap();
            let snapshot = scale.snapshot(now);
            // Эталон: палец прошёл 2·frame pt в сторону роста — контент ровно на столько же.
            assert!(
                (snapshot.offset_points - (start + 2.0 * frame as f64)).abs() < 1e-9,
                "{direction:?}, кадр {frame}: смещение {}",
                snapshot.offset_points
            );
            assert_eq!(snapshot.phase, MotionPhase::Touched);
        }
    }
}

#[test]
fn vertical_scale_reads_only_the_vertical_finger_component() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(
        0.0,
        exposure_spec(GrowthDirection::FingerUp, DragOrigin::FromRecognition),
        &tokens,
    );
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    let translation = ui_core::gesture::PanTranslation {
        from_touch_down: ui_core::geometry::Vector::new(40.0, -20.0),
        from_recognition: ui_core::geometry::Vector::new(40.0, -20.0),
    };
    scale.drag_moved(ms(16), translation).unwrap();
    // Палец вверх на 20 pt при FingerUp — +20; горизонтальные 40 pt игнорируются.
    assert!((scale.snapshot(ms(16)).offset_points - 20.0).abs() < 1e-9);
}

#[test]
fn drag_start_has_no_jump_on_hysteresis_when_measuring_from_recognition() {
    let tokens = UiKitTokens::default();
    // Палец уже прошёл 8 pt гистерезиса до распознавания.
    let mut smooth = exposure_scale(
        0.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition),
        &tokens,
    );
    let before = smooth.snapshot(ms(0)).offset_points;
    smooth.drag_began(ms(0), pan(8.0, 0.0)).unwrap();
    assert_eq!(
        smooth.snapshot(ms(0)).offset_points,
        before,
        "на Began контент не прыгает"
    );
    smooth.drag_moved(ms(16), pan(18.0, 10.0)).unwrap();
    assert!((smooth.snapshot(ms(16)).offset_points - (before + 10.0)).abs() < 1e-9);

    // Вариант «от касания» делает скачок на гистерезис — это и есть то, чего избегаем.
    let mut jumping = exposure_scale(
        0.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromTouchDown),
        &tokens,
    );
    jumping.drag_began(ms(0), pan(8.0, 0.0)).unwrap();
    assert!((jumping.snapshot(ms(0)).offset_points - (before + 8.0)).abs() < 1e-9);
    jumping.drag_moved(ms(16), pan(18.0, 10.0)).unwrap();
    assert!((jumping.snapshot(ms(16)).offset_points - (before + 18.0)).abs() < 1e-9);
}

#[test]
fn first_sample_after_release_has_the_finger_velocity_without_a_step() {
    let tokens = UiKitTokens::default();
    let cases: [(GrowthDirection, f64, f64); 3] = [
        (GrowthDirection::FingerRight, 220.0, 220.0),
        (GrowthDirection::FingerLeft, -220.0, 220.0),
        (GrowthDirection::FingerLeft, 220.0, -220.0),
    ];
    for (direction, finger_velocity, content_velocity) in cases {
        let mut scale = exposure_scale(0.0, exposure_spec(direction, DragOrigin::FromRecognition), &tokens);
        scale.drag_began(ms(0), pan_same(0.0)).unwrap();
        scale
            .drag_moved(ms(16), pan_same(finger_velocity.signum() * 20.0))
            .unwrap();
        let offset_before = scale.snapshot(ms(16)).offset_points;
        scale.drag_ended(ms(16), horizontal(finger_velocity)).unwrap();
        // Положение в момент отпускания не изменилось, скорость контента = скорости пальца.
        assert!((scale.snapshot(ms(16)).offset_points - offset_before).abs() < 1e-9);
        assert!(
            (scale.velocity_points_per_second(ms(16)) - content_velocity).abs() < 1e-6,
            "{direction:?}: скорость контента {}",
            scale.velocity_points_per_second(ms(16))
        );
        assert_eq!(scale.snapshot(ms(16)).phase, MotionPhase::Settling);
    }
}

#[test]
fn release_velocity_is_inherited_by_the_magnet_spring_too() {
    let tokens = UiKitTokens::default();
    // Якорь 1,5 стоит на 168,45 pt; бросок из 100 pt со скоростью 150 pt/с проецируется в ≈ 174,85 —
    // внутри радиуса магнита (14 pt).
    let mut scale = magnet_scale(1.0, vec![0.5, 1.0, 1.5], 4.0, &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(100.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(150.0)).unwrap();
    assert!(
        (scale.velocity_points_per_second(ms(16)) - 150.0).abs() < 1e-6,
        "магнит стартует со скоростью пальца, ступеньки нет: {}",
        scale.velocity_points_per_second(ms(16))
    );
}

#[test]
fn touching_a_moving_scale_stops_it_exactly_where_it_is() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(
        -2.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition),
        &tokens,
    );
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(30.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(400.0)).unwrap();
    let catch_time = ms(16 + 150);
    let before = scale.snapshot(catch_time);
    assert_eq!(before.phase, MotionPhase::Settling, "в момент касания шкала едет");
    assert!(scale.velocity_points_per_second(catch_time).abs() > 10.0);

    scale.catch(catch_time);
    let after = scale.snapshot(catch_time);
    assert!(
        (after.offset_points - before.offset_points).abs() < 1e-9,
        "положение до и после захвата совпадает"
    );
    assert_eq!(after.phase, MotionPhase::Touched);
    // И остаётся там, сколько бы ни держали палец.
    for later in [100, 1_000, 10_000] {
        let held = scale.snapshot(ms(16 + 150 + later));
        assert!((held.offset_points - before.offset_points).abs() < 1e-9);
        assert_eq!(scale.velocity_points_per_second(ms(16 + 150 + later)), 0.0);
    }
    // Палец поднят без движения: шкала доезжает до ближайшего шага.
    let release_time = ms(5_000);
    scale.release_without_drag(release_time).unwrap();
    let settled = scale.snapshot(ms(20_000));
    let expected_step = (before.offset_points / STEP_POINTS).round();
    assert!((settled.offset_points - expected_step * STEP_POINTS).abs() < 1e-9);
    assert_eq!(settled.phase, MotionPhase::Resting);
}

#[test]
fn dragging_past_the_edge_uses_the_rubber_band_and_returns_without_a_jump() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(
        2.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition),
        &tokens,
    );
    let edge = scale.snapshot(ms(0)).offset_points;
    assert!((edge - 72.0).abs() < 1e-9);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    // Внутри границ 1:1, за краем — формула «резинки» из motion.md (c = 0,55, d = размер области).
    scale.drag_moved(ms(16), pan_same(200.0)).unwrap();
    let stretched = scale.snapshot(ms(16)).offset_points;
    assert!(
        (stretched - (edge + rubber_formula(200.0, VIEWPORT_POINTS))).abs() < 1e-9,
        "резинка: {stretched}"
    );
    assert!(stretched < edge + 200.0, "контент едет медленнее пальца");
    // Значение за краем остаётся крайним.
    assert!((scale.snapshot(ms(16)).value - 2.0).abs() < 1e-12);

    // Отпустили без скорости: плотная выборка 1 мс — ни одного скачка, возврат точно в границу.
    scale.drag_ended(ms(16), horizontal(0.0)).unwrap();
    let mut previous = scale.snapshot(ms(16)).offset_points;
    assert!(
        (previous - stretched).abs() < 1e-9,
        "в момент отпускания положение то же"
    );
    for step in 1..=2_000 {
        let current = scale.snapshot(ms(16 + step)).offset_points;
        assert!(
            (current - previous).abs() < 3.0,
            "скачок {previous} → {current} на {step} мс"
        );
        previous = current;
    }
    assert!((previous - edge).abs() < 1e-9);
}

#[test]
fn catching_a_scale_that_is_returning_from_the_edge_does_not_jump() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(
        2.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition),
        &tokens,
    );
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(150.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(0.0)).unwrap();
    let catch_time = ms(16 + 120);
    let before = scale.snapshot(catch_time).offset_points;
    assert!(before > 72.0, "ещё за краем: {before}");
    scale.catch(catch_time);
    assert!((scale.snapshot(catch_time).offset_points - before).abs() < 1e-9);
    // Новое перетаскивание продолжает с этого положения (резинка обратима).
    scale.drag_began(ms(300), pan_same(0.0)).unwrap();
    assert!((scale.snapshot(ms(300)).offset_points - before).abs() < 1e-9);
}

#[test]
fn flick_snaps_to_the_step_predicted_by_the_deceleration_projection() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(
        -2.0,
        exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition),
        &tokens,
    );
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(30.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(100.0)).unwrap();
    // Независимый эталон motion.md раздел 2: проекция = (v/1000)·d/(1−d), d = 0,998 → ≈ 0,499·v.
    let projected = -72.0 + 30.0 + 100.0 / 1000.0 * 0.998 / (1.0 - 0.998);
    let expected_step = (projected / STEP_POINTS).round();
    // Кадры по 16 мс, как это делает экран: событие «успокоилась» приходит с кадром.
    for frame in 1..=600 {
        scale.advance(ms(16 + frame * 16));
    }
    let settled = scale.snapshot(ms(16 + 600 * 16));
    assert_eq!(settled.phase, MotionPhase::Resting);
    assert!((settled.offset_points - expected_step * STEP_POINTS).abs() < 1e-9);
    assert!(
        (settled.value - expected_step / 3.0).abs() < 1e-12,
        "значение {}",
        settled.value
    );
    let events = scale.drain_events();
    assert!(
        events.iter().any(
            |event| matches!(event, ScaleEvent::Settled { value } if (*value - expected_step / 3.0).abs() < 1e-12)
        ),
        "{events:?}"
    );
}

#[test]
fn reduced_motion_scale_settles_without_overshoot() {
    let tokens = UiKitTokens::default();
    let spec = exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition);
    let mut scale = ui_kit::scale::SnapScale::new(
        exposure_model(),
        0.0,
        spec,
        &env(&tokens, MotionPreference::Reduced),
        ms(0),
    )
    .unwrap();
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(10.0)).unwrap();
    scale.drag_ended(ms(16), horizontal(0.0)).unwrap();
    // Цель — шаг 12 pt; перелёта (> 12) быть не должно.
    let peak = (0..3_000)
        .map(|t| scale.snapshot(ms(16 + t)).offset_points)
        .fold(f64::MIN, f64::max);
    assert!(peak <= 12.0 + 1e-9, "перелёт при Reduced: {peak}");
}
