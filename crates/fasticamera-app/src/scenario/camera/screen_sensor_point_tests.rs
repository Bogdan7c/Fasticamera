//! Независимое правило «тап → точка сенсора»: все четыре ориентации, граница допуска и отказы без точки.

use camera_contract::three_a::{MeteringRegion, PointRequestId, SensorPoint};

use super::*;

fn point_at(x: f32, y: f32) -> PointOfInterest {
    PointOfInterest::Point {
        region: MeteringRegion::new(SensorPoint::new(x, y).expect("точка"), 0.1).expect("регион"),
        request: PointRequestId(1),
    }
}

#[test]
fn the_rule_matches_the_hand_derived_case_of_the_end_to_end_test() {
    // Тап (u = 0,25; v = 0,75) при сенсоре 90°: сенсор (v, 1 − u) = (0,75; 0,75).
    let expected = expected_sensor_point(FramePoint { u: 0.25, v: 0.75 }, SensorOrientation::Degrees90);
    assert_eq!(expected, SensorFraction { x: 0.75, y: 0.75 });
}

#[test]
fn every_orientation_rotates_the_corner_tap_to_a_different_sensor_corner() {
    let frame = FramePoint { u: 0.25, v: 0.25 };
    let corners = [
        (SensorOrientation::Degrees0, (0.25, 0.25)),
        (SensorOrientation::Degrees90, (0.25, 0.75)),
        (SensorOrientation::Degrees180, (0.75, 0.75)),
        (SensorOrientation::Degrees270, (0.75, 0.25)),
    ];
    for (orientation, (x, y)) in corners {
        assert_eq!(
            expected_sensor_point(frame, orientation),
            SensorFraction { x, y },
            "{orientation:?}"
        );
    }
}

#[test]
fn the_rule_is_inverse_to_rotating_the_sensor_image_clockwise() {
    // Сенсорная точка при повороте кадра на θ по часовой встаёт в экранную точку: проверяем обратимость.
    let rotate_to_screen = |orientation: SensorOrientation, sensor: SensorFraction| match orientation {
        SensorOrientation::Degrees0 => (sensor.x, sensor.y),
        SensorOrientation::Degrees90 => (1.0 - sensor.y, sensor.x),
        SensorOrientation::Degrees180 => (1.0 - sensor.x, 1.0 - sensor.y),
        SensorOrientation::Degrees270 => (sensor.y, 1.0 - sensor.x),
    };
    for orientation in [
        SensorOrientation::Degrees0,
        SensorOrientation::Degrees90,
        SensorOrientation::Degrees180,
        SensorOrientation::Degrees270,
    ] {
        let frame = FramePoint { u: 0.2, v: 0.7 };
        let (u, v) = rotate_to_screen(orientation, expected_sensor_point(frame, orientation));
        assert!(
            (u - frame.u).abs() < 1e-6 && (v - frame.v).abs() < 1e-6,
            "{orientation:?}: ({u}, {v})"
        );
    }
}

#[test]
fn check_point_separates_match_off_no_point_and_unknown_orientation() {
    let frame = FramePoint { u: 0.25, v: 0.75 };
    let ninety = Some(SensorOrientation::Degrees90);
    assert_eq!(
        check_point(point_at(0.75, 0.75), ninety, frame, 0.05),
        PointCheck::Matches
    );
    assert_eq!(
        check_point(point_at(0.79, 0.71), ninety, frame, 0.05),
        PointCheck::Matches
    );
    assert_eq!(
        check_point(point_at(0.75, 0.25), ninety, frame, 0.05),
        PointCheck::Off {
            expected: SensorFraction { x: 0.75, y: 0.75 },
            actual: SensorFraction { x: 0.75, y: 0.25 },
        }
    );
    assert_eq!(
        check_point(PointOfInterest::WholeFrame, ninety, frame, 0.05),
        PointCheck::NoPoint
    );
    assert_eq!(
        check_point(point_at(0.75, 0.75), None, frame, 0.05),
        PointCheck::UnknownOrientation
    );
}
