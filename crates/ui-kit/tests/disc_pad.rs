#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Площадка «точка в круге»: относительное ведение, ось y вверх, прижатие к краю, действие рождает только палец.

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_kit::disc_pad::{DiscPad, DiscPadError, DiscPadSpec, DiscPoint, DiscPointError, ProgrammaticSet};
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::UiKitTokens;

const RADIUS_PT: f64 = 100.0;
const TOLERANCE: f64 = 1e-9;

fn pad_at(initial: DiscPoint) -> DiscPad {
    let tokens = UiKitTokens::default();
    DiscPad::new(DiscPadSpec::with_token_drive(RADIUS_PT, &tokens.disc_pad), initial).unwrap()
}

fn translation(dx: f64, dy: f64) -> PanTranslation {
    let shift = Vector::new(dx, dy);
    PanTranslation {
        from_touch_down: shift,
        from_recognition: shift,
    }
}

fn began() -> PanUpdate {
    PanUpdate::Began {
        translation: translation(0.0, 0.0),
    }
}

fn changed(dx: f64, dy: f64) -> PanUpdate {
    PanUpdate::Changed {
        translation: translation(dx, dy),
    }
}

fn ended() -> PanUpdate {
    PanUpdate::Ended { velocity: Vector::ZERO }
}

#[test]
fn finger_drag_moves_point_with_ratio_and_flipped_y() {
    let mut pad = pad_at(DiscPoint::CENTER);
    assert_eq!(pad.apply_pan(began()).unwrap(), None, "начало жеста точку не двигает");
    // Палец вправо на 40 pt и ВНИЗ на 20 pt (экран, y вниз): точка = (40·0,5/100, −20·0,5/100) = (0,2, −0,1).
    let moved = pad
        .apply_pan(changed(40.0, 20.0))
        .unwrap()
        .expect("значение изменилось");
    assert!((moved.x() - 0.2).abs() < TOLERANCE, "x = {}", moved.x());
    assert!(
        (moved.y() + 0.1).abs() < TOLERANCE,
        "палец вниз = точка вниз (y < 0), y = {}",
        moved.y()
    );
    assert_eq!(pad.point(), moved);
    // Палец ВВЕРХ (dy < 0) — точка вверх.
    let up = pad.apply_pan(changed(0.0, -60.0)).unwrap().unwrap();
    assert!((up.y() - 0.3).abs() < TOLERANCE);
}

#[test]
fn drag_from_nonzero_point_is_relative_and_does_not_jump() {
    let start = DiscPoint::clamped(0.5, 0.25).unwrap();
    let mut pad = pad_at(start);
    // Палец лёг в совсем другом месте круга: на Began точка остаётся на месте.
    assert_eq!(pad.apply_pan(began()).unwrap(), None);
    assert_eq!(pad.point(), start);
    let moved = pad.apply_pan(changed(20.0, 0.0)).unwrap().unwrap();
    assert!((moved.x() - 0.6).abs() < TOLERANCE);
    assert!((moved.y() - 0.25).abs() < TOLERANCE);
    // Новое ведение стартует от текущей точки, а не от нуля.
    pad.apply_pan(ended()).unwrap();
    pad.apply_pan(began()).unwrap();
    let again = pad.apply_pan(changed(20.0, 0.0)).unwrap().unwrap();
    assert!((again.x() - 0.7).abs() < TOLERANCE);
}

#[test]
fn pushing_past_the_edge_clamps_at_same_angle_and_does_not_repeat() {
    let mut pad = pad_at(DiscPoint::CENTER);
    pad.apply_pan(began()).unwrap();
    // Огромное смещение по диагонали вправо-вверх: на краю, угол 45°.
    let edge = pad.apply_pan(changed(1000.0, -1000.0)).unwrap().expect("дошли до края");
    assert!((edge.length() - 1.0).abs() < TOLERANCE);
    assert!(
        (edge.x() - edge.y()).abs() < TOLERANCE && edge.x() > 0.0,
        "угол 45° сохранён: {edge:?}"
    );
    // Палец тянет дальше по тому же лучу — точка на месте, действия нет.
    assert_eq!(pad.apply_pan(changed(2000.0, -2000.0)).unwrap(), None);
    assert_eq!(pad.point(), edge);
    // Вернули палец — точка уходит с края как относительное ведение от НАЧАЛА жеста.
    let back = pad.apply_pan(changed(0.0, 0.0)).unwrap().expect("вернулись к началу");
    assert_eq!(back, DiscPoint::CENTER);
}

#[test]
fn ended_and_cancelled_finish_drag_and_keep_value() {
    for finish in [ended(), PanUpdate::Cancelled] {
        let mut pad = pad_at(DiscPoint::CENTER);
        pad.apply_pan(began()).unwrap();
        let moved = pad.apply_pan(changed(40.0, 0.0)).unwrap().unwrap();
        assert!(pad.is_finger_driven());
        assert_eq!(pad.apply_pan(finish).unwrap(), None, "конец жеста действия не рождает");
        assert!(!pad.is_finger_driven());
        assert_eq!(pad.point(), moved, "точка остаётся где есть");
        // После конца запоздалый Changed без Began игнорируется.
        assert_eq!(pad.apply_pan(changed(80.0, 0.0)).unwrap(), None);
        assert_eq!(pad.point(), moved);
    }
}

#[test]
fn programmatic_set_applies_when_idle_and_is_dropped_while_finger_drives() {
    let mut pad = pad_at(DiscPoint::CENTER);
    let target = DiscPoint::clamped(-0.4, 0.3).unwrap();
    assert_eq!(pad.set_point(target), ProgrammaticSet::Applied);
    assert_eq!(pad.point(), target);
    assert_eq!(pad.dropped_programmatic_sets(), 0);

    pad.apply_pan(began()).unwrap();
    let foreign = DiscPoint::clamped(0.9, 0.0).unwrap();
    assert_eq!(pad.set_point(foreign), ProgrammaticSet::DroppedWhileFingerDrives);
    assert_eq!(pad.point(), target, "палец не перебит");
    assert_eq!(pad.dropped_programmatic_sets(), 1);
    // Ведение продолжается от точки начала жеста, а не от чужой установки.
    let moved = pad.apply_pan(changed(20.0, 0.0)).unwrap().unwrap();
    assert!((moved.x() - (-0.3)).abs() < TOLERANCE);

    pad.apply_pan(ended()).unwrap();
    assert_eq!(
        pad.set_point(foreign),
        ProgrammaticSet::Applied,
        "после отпускания установка снова работает"
    );
    assert_eq!(pad.point(), foreign);
}

#[test]
fn non_finite_input_is_a_distinct_error_and_leaves_state_untouched() {
    assert!(matches!(
        DiscPoint::clamped(f64::NAN, 0.0),
        Err(DiscPointError::NotFinite { .. })
    ));
    assert!(matches!(
        DiscPoint::clamped(0.0, f64::INFINITY),
        Err(DiscPointError::NotFinite { .. })
    ));

    let mut pad = pad_at(DiscPoint::CENTER);
    pad.apply_pan(began()).unwrap();
    let good = pad.apply_pan(changed(20.0, 0.0)).unwrap().unwrap();
    assert!(pad.apply_pan(changed(f64::NAN, 0.0)).is_err());
    assert_eq!(pad.point(), good, "ошибка не меняет точку");
}

#[test]
fn point_constructor_clamps_outside_and_keeps_inside() {
    let inside = DiscPoint::clamped(0.3, -0.4).unwrap();
    assert_eq!((inside.x(), inside.y()), (0.3, -0.4));
    let outside = DiscPoint::clamped(3.0, 4.0).unwrap();
    assert!((outside.x() - 0.6).abs() < TOLERANCE && (outside.y() - 0.8).abs() < TOLERANCE);
    assert_eq!(DiscPoint::CENTER.length(), 0.0);
}

#[test]
fn invalid_spec_is_rejected() {
    for radius_pt in [0.0, -5.0, f64::NAN] {
        let spec = DiscPadSpec {
            radius_pt,
            drive_ratio: 0.5,
        };
        assert!(matches!(
            DiscPad::new(spec, DiscPoint::CENTER),
            Err(DiscPadError::InvalidRadius(_))
        ));
    }
    let spec = DiscPadSpec {
        radius_pt: 100.0,
        drive_ratio: 0.0,
    };
    assert!(matches!(
        DiscPad::new(spec, DiscPoint::CENTER),
        Err(DiscPadError::InvalidDriveRatio(_))
    ));
}

#[test]
fn drive_ratio_comes_from_token_default_half() {
    let tokens = UiKitTokens::default();
    assert_eq!(tokens.disc_pad.drive_ratio, 0.5);
    assert!(tokens.validate().is_ok());
}

#[test]
fn clamped_points_always_satisfy_the_unit_circle_criterion_strictly() {
    let mut checked = 0_u32;
    for angle_step in 0..3600 {
        let angle = f64::from(angle_step) * 0.1_f64.to_radians() + 0.000_123;
        for scale in [1.000_000_000_000_1, 1.01, 1.5, 3.0, 1e6, 1e150, 1e300, f64::MAX / 4.0] {
            let point = DiscPoint::clamped(angle.cos() * scale, angle.sin() * scale).unwrap();
            assert!(
                point.x() * point.x() + point.y() * point.y() <= 1.0,
                "за кругом: ({}, {}) при угле {angle}, масштабе {scale}",
                point.x(),
                point.y()
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 3600 * 8);
}

#[test]
fn drag_uses_translation_from_recognition_not_from_touch_down() {
    let mut pad = pad_at(DiscPoint::CENTER);
    let shifted = |touch_down_dx: f64, recognition_dx: f64| PanTranslation {
        from_touch_down: Vector::new(touch_down_dx, 0.0),
        from_recognition: Vector::new(recognition_dx, 0.0),
    };
    pad.apply_pan(PanUpdate::Began {
        translation: shifted(50.0, 0.0),
    })
    .unwrap();
    assert_eq!(pad.point(), DiscPoint::CENTER, "порог распознавания не даёт скачка");
    let moved = pad
        .apply_pan(PanUpdate::Changed {
            translation: shifted(50.0, 10.0),
        })
        .unwrap()
        .unwrap();
    // От распознавания: 10 pt · 0,5 / 100 = 0,05 (от касания было бы 0,25).
    assert!((moved.x() - 0.05).abs() < TOLERANCE, "x = {}", moved.x());
}
