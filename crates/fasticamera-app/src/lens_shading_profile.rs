//! Калибровка виньетирования камер устройства: вшитый в сборку профиль → набор для бэкенда камеры.
//!
//! Профиль — JSON схемы `fasticamera.lens-shading-calibration/1` из `device-profiles/<устройство>/`
//! (его снимает и пишет стенд `tools/device-probe`). Он вшит в бинарь через `include_str!`, поэтому
//! приложению не нужны файлы на телефоне, а бэкенд камеры (`camera-android`) ничего не знает ни о JSON,
//! ни об устройствах: он получает готовый набор [`LensShadingCalibrations`].
//!
//! # Правила разбора
//! * неизвестные поля запрещены везде, кроме информационного блока `provenance` (откуда числа);
//! * неверная схема, пустой ID камеры, повтор камеры, битая сетка — явная ошибка
//!   ([`LensShadingProfileError`]); приложение не стартует с битым вшитым профилем, а не работает
//!   молча без коррекции;
//! * профиль чужого устройства — не ошибка, а пустой набор с причиной `DeviceNotInProfile` и записью
//!   в лог: на другом телефоне чужую калибровку применять нельзя.

use camera_contract::geometry::PixelSize;
use camera_contract::ids::{CameraId, EmptyCameraId};
use camera_contract::lens_shading::{
    InvalidGainGrid, LensShadingCalibration, LensShadingCalibrations, LensShadingGainGrid,
};
use serde::Deserialize;

use crate::device_config::DeviceConfig;

/// Профиль основного устройства разработки (OnePlus 11), вшитый в сборку.
const EMBEDDED_PROFILE_JSON: &str = include_str!("../../../device-profiles/oneplus-cph2447/lens-shading.json");

/// Единственная поддерживаемая версия схемы файла.
const SUPPORTED_SCHEMA: &str = "fasticamera.lens-shading-calibration/1";

/// Что не так с профилем калибровки.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LensShadingProfileError {
    #[error("профиль калибровки не разобран: {0}")]
    Malformed(String),
    #[error("схема профиля `{found}` не поддерживается (ожидали `{SUPPORTED_SCHEMA}`)")]
    UnsupportedSchema { found: String },
    #[error("пустой ID камеры в профиле: {0}")]
    EmptyCameraId(#[from] EmptyCameraId),
    #[error("камера {camera} встречается в профиле дважды")]
    DuplicateCamera { camera: String },
    #[error("камера {camera}: активная область {width}×{height} пуста")]
    EmptyActiveArray { camera: String, width: u32, height: u32 },
    #[error("камера {camera}: сетка усилений: {source}")]
    InvalidGrid {
        camera: String,
        #[source]
        source: InvalidGainGrid,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    schema: String,
    device: ProfileDevice,
    cameras: Vec<ProfileCamera>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDevice {
    manufacturer: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileCamera {
    camera_id: String,
    active_array: ProfileSize,
    grid: ProfileGrid,
    /// Откуда числа (стенд, укладки, подгонка): свободная форма, читателем не используется.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "provenance — свободная форма для людей; здесь только разрешён схемой"
    )]
    provenance: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileSize {
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileGrid {
    columns: u32,
    rows: u32,
    points: Vec<[f32; 4]>,
}

/// Калибровки вшитого профиля для устройства `device` (из конфига нативной части).
pub fn embedded_lens_shading(device: &DeviceConfig) -> Result<LensShadingCalibrations, LensShadingProfileError> {
    parse_lens_shading_profile(EMBEDDED_PROFILE_JSON, device)
}

/// Разбирает профиль `json` и отбирает его для устройства `device`.
///
/// Файл разбирается и проверяется целиком ДО сверки устройства: битый профиль — ошибка на любом телефоне.
pub fn parse_lens_shading_profile(
    json: &str,
    device: &DeviceConfig,
) -> Result<LensShadingCalibrations, LensShadingProfileError> {
    let file: ProfileFile =
        serde_json::from_str(json).map_err(|error| LensShadingProfileError::Malformed(error.to_string()))?;
    if file.schema != SUPPORTED_SCHEMA {
        return Err(LensShadingProfileError::UnsupportedSchema { found: file.schema });
    }
    let calibrations = file
        .cameras
        .into_iter()
        .map(convert_camera)
        .collect::<Result<Vec<_>, _>>()?;
    for (index, calibration) in calibrations.iter().enumerate() {
        if calibrations[..index]
            .iter()
            .any(|earlier| earlier.camera == calibration.camera)
        {
            return Err(LensShadingProfileError::DuplicateCamera {
                camera: calibration.camera.to_string(),
            });
        }
    }
    if !device_matches(&file.device, device) {
        tracing::info!(
            target: "fasticamera::lens_shading_profile",
            profile_manufacturer = %file.device.manufacturer,
            profile_model = %file.device.model,
            device_manufacturer = ?device.make,
            device_model = ?device.model,
            "профиль калибровки виньетирования снят на другом устройстве: коррекция выключена"
        );
        return Ok(LensShadingCalibrations::device_not_in_profile());
    }
    Ok(LensShadingCalibrations::new(calibrations))
}

fn convert_camera(camera: ProfileCamera) -> Result<LensShadingCalibration, LensShadingProfileError> {
    let id = CameraId::new(camera.camera_id.as_str())?;
    let ProfileSize { width, height } = camera.active_array;
    if width == 0 || height == 0 {
        return Err(LensShadingProfileError::EmptyActiveArray {
            camera: camera.camera_id,
            width,
            height,
        });
    }
    let grid =
        LensShadingGainGrid::new(camera.grid.columns, camera.grid.rows, camera.grid.points).map_err(|source| {
            LensShadingProfileError::InvalidGrid {
                camera: camera.camera_id,
                source,
            }
        })?;
    Ok(LensShadingCalibration {
        camera: id,
        active_array: PixelSize::new(width, height),
        grid,
    })
}

/// Совпадает ли устройство с тем, на котором снят профиль: производитель без учёта регистра
/// (Android отдаёт `OnePlus`, но регистр у производителей разный), модель — точно, после обрезки пробелов.
/// Неизвестное устройство (Kotlin не прислал `device`) не совпадает ни с чем.
fn device_matches(profile: &ProfileDevice, device: &DeviceConfig) -> bool {
    let (Some(make), Some(model)) = (device.make.as_deref(), device.model.as_deref()) else {
        return false;
    };
    profile.manufacturer.trim().eq_ignore_ascii_case(make.trim()) && profile.model.trim() == model.trim()
}

#[cfg(test)]
#[path = "lens_shading_profile_tests.rs"]
mod tests;
