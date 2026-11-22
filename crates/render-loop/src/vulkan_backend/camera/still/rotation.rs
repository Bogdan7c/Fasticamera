//! Поворот снимка «к тому, как держали телефон» (тег ориентации файлов; пиксели не вращаются).
//!
//! Формула Android `CaptureRequest.JPEG_ORIENTATION` (документация `CameraCharacteristics.SENSOR_ORIENTATION`):
//! `(ориентация сенсора + знак · поворот телефона) mod 360`, где поворот телефона — по часовой стрелке от
//! естественного портрета, а знак: задняя камера `+1`, фронтальная `-1` (кадр фронтальной камеры не
//! зеркалится в файле: зеркало — забота показа, а не снимка).
//!
//! `DeviceOrientation` назван как `UIDeviceOrientation` iOS: `LandscapeLeft` — телефон повёрнут на 90°
//! ПРОТИВ часовой стрелки (верх смотрит влево), то есть 270° по часовой; `LandscapeRight` — 90° по часовой.

use camera_contract::geometry::SensorOrientation;
use camera_contract::ids::LensFacing;
use shell_contract::orientation::DeviceOrientation;
use still_contract::shot::StillRotation;

/// Поворот телефона по часовой стрелке от естественного портрета, четверти оборота (0–3).
fn device_quarter_turns_clockwise(orientation: DeviceOrientation) -> i32 {
    match orientation {
        DeviceOrientation::Portrait => 0,
        DeviceOrientation::LandscapeRight => 1,
        DeviceOrientation::PortraitUpsideDown => 2,
        DeviceOrientation::LandscapeLeft => 3,
    }
}

/// Поворот картинки в ориентации сенсора, после которого она «как держали телефон».
pub fn still_rotation(sensor: SensorOrientation, facing: LensFacing, device: DeviceOrientation) -> StillRotation {
    let sensor_turns = i32::try_from(sensor.quarter_turns_clockwise()).unwrap_or(0);
    let device_turns = device_quarter_turns_clockwise(device);
    let signed_device_turns = match facing {
        LensFacing::Front => -device_turns,
        // Внешняя камера ведёт себя как задняя: зеркала у неё нет.
        LensFacing::Back | LensFacing::External => device_turns,
    };
    // `from_degrees` принимает любой знак; кратность 90° здесь гарантирована построением.
    StillRotation::from_degrees((sensor_turns + signed_device_turns) * 90).unwrap_or(StillRotation::None)
}

#[cfg(test)]
#[path = "rotation_tests.rs"]
mod tests;
