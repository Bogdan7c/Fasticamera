//! Таблица поворота: 4 ориентации телефона × 2 стороны × ориентация сенсора OnePlus 11 (90° и 270°).

use super::*;
use DeviceOrientation::{LandscapeLeft, LandscapeRight, Portrait, PortraitUpsideDown};
use StillRotation::{Clockwise90, Clockwise180, Clockwise270, None as NoRotation};

fn degrees(sensor: SensorOrientation, facing: LensFacing, device: DeviceOrientation) -> u32 {
    still_rotation(sensor, facing, device).degrees()
}

#[test]
fn back_camera_adds_the_device_rotation_to_the_sensor_orientation() {
    // OnePlus 11, задняя камера: сенсор 90°.
    let table = [
        (Portrait, Clockwise90),
        (LandscapeRight, Clockwise180),
        (PortraitUpsideDown, Clockwise270),
        (LandscapeLeft, NoRotation),
    ];
    for (device, expected) in table {
        assert_eq!(
            still_rotation(SensorOrientation::Degrees90, LensFacing::Back, device),
            expected,
            "задняя, {device:?}"
        );
    }
}

#[test]
fn front_camera_subtracts_the_device_rotation() {
    // OnePlus 11, фронтальная: сенсор 270°.
    let table = [
        (Portrait, Clockwise270),
        (LandscapeRight, Clockwise180),
        (PortraitUpsideDown, Clockwise90),
        (LandscapeLeft, NoRotation),
    ];
    for (device, expected) in table {
        assert_eq!(
            still_rotation(SensorOrientation::Degrees270, LensFacing::Front, device),
            expected,
            "фронтальная, {device:?}"
        );
    }
}

#[test]
fn portrait_back_matches_the_rotation_the_viewfinder_applies() {
    // Видоискатель в портрете поворачивает кадр ровно на ориентацию сенсора (`ViewfinderGeometry`).
    for sensor in [
        SensorOrientation::Degrees0,
        SensorOrientation::Degrees90,
        SensorOrientation::Degrees180,
        SensorOrientation::Degrees270,
    ] {
        assert_eq!(
            degrees(sensor, LensFacing::Back, Portrait),
            sensor.quarter_turns_clockwise() * 90
        );
    }
}

#[test]
fn external_camera_behaves_like_the_back_one() {
    for device in [Portrait, LandscapeLeft, LandscapeRight, PortraitUpsideDown] {
        assert_eq!(
            still_rotation(SensorOrientation::Degrees90, LensFacing::External, device),
            still_rotation(SensorOrientation::Degrees90, LensFacing::Back, device)
        );
    }
}
