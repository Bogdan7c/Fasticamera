//! Зона держания (магнит нуля) на уровне привода оси: ручка, стоящая на нуле «с точностью до арифметики», обязана
//! стартовать без мёртвого хода в обе стороны (решение 247).

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::{MonotonicNanos, MotionPreference};

use super::*;
use crate::tokens::UiKitTokens;

const HALF_WIDTH_PT: f64 = 6.0;

fn now() -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000)
}

fn finger_right_by(pt: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(pt, 0.0),
        from_recognition: Vector::new(pt, 0.0),
    }
}

/// Привод с магнитом на нуле, контент стоит на смещении `rest_offset`.
fn drive_resting_at(rest_offset: f64) -> AxisDrive {
    let tokens = UiKitTokens::default();
    let environment = ElementEnvironment {
        tokens: &tokens,
        motion: MotionPreference::Full,
        viewport_extent_points: 300.0,
    };
    let setup = DriveSetup {
        role: SnapRole::Scale,
        direction: GrowthDirection::FingerLeft,
        drag_origin: DragOrigin::FromTouchDown,
    };
    let mut drive = AxisDrive::from_environment(
        &environment,
        setup,
        OffsetRange {
            min: -200.0,
            max: 200.0,
        },
        rest_offset,
    )
    .unwrap();
    drive.set_hold_zone(Some(HoldZone {
        centre_offset: 0.0,
        half_width: HALF_WIDTH_PT,
    }));
    drive
}

#[test]
fn a_knob_resting_a_hair_off_the_zero_centre_starts_without_dead_travel_in_both_directions() {
    for rest_error in [1e-12_f64, -1e-12, 0.0] {
        for finger_pt in [1.0_f64, -1.0, 4.0, -4.0, 11.0, -11.0] {
            let mut drive = drive_resting_at(rest_error);
            drive.catch(now());
            drive.drag_began(now(), finger_right_by(0.0)).unwrap();
            drive.drag_moved(finger_right_by(finger_pt)).unwrap();
            let moved = drive.sample(now()).offset - rest_error;
            // Палец вправо уменьшает смещение (`FingerLeft` растёт влево): путь контента равен пути пальца.
            assert!(
                (moved + finger_pt).abs() < 1e-6,
                "ошибка покоя {rest_error}, палец {finger_pt}: контент идёт за пальцем 1:1, а прошёл {moved}"
            );
        }
    }
}

#[test]
fn the_centre_check_has_a_documented_tolerance_but_not_a_loose_one() {
    let zone = HoldZone {
        centre_offset: 0.0,
        half_width: HALF_WIDTH_PT,
    };
    assert!(zone.is_on_centre(1e-12));
    assert!(zone.is_on_centre(-1e-12));
    assert!(!zone.is_on_centre(0.5), "полпункта сбоку — это уже сторона, а не центр");
    assert!(!zone.is_on_centre(-0.5));
}
