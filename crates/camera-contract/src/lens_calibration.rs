//! Геометрическая калибровка объектива (сессия 13): где у камеры оптическая ось и как камера
//! стоит в корпусе относительно основной.
//!
//! Зачем: при переходе 0.6× → 1× картинка не должна «прыгать». Центр кадрирования ширика ставится
//! туда, куда смотрит оптическая ось камеры (`ACAMERA_LENS_INTRINSIC_CALIBRATION`, `c_x, c_y`), а не
//! в геометрический центр сенсора. На OnePlus 11 у ширика ось смещена на ≈ 240 px из 4000 (6 %),
//! у основной — на ≈ 8 и 21 px (снято `dumpsys media.camera` в начале 13).
//!
//! # Правдоподобие
//! HAL отдаёт и заглушки: у фронтальной камеры OnePlus 11 `c_x = c_y = 0` (ось в углу сенсора —
//! физически невозможно). Поэтому калибровка сначала проверяется ([`LensCalibration::optical_axis`]):
//! неправдоподобная — это честное «калибровки нет» с причиной, и кадрирование остаётся по центру.
//! Угадывать ось вместо заглушки нельзя (инвариант 6 README: не подменять молча).
//!
//! Поворот и сдвиг камеры (`poseRotation`/`poseTranslation`) хранятся для будущего (параллакс по
//! дистанции фокуса — бэклог), но в выравнивании 13 не участвуют: сдвиг 17 мм даёт параллакс,
//! зависящий от расстояния до объекта, а поворот между задними камерами ≈ 0,5° (≈ 8 px на ширике) —
//! на порядок меньше смещения оси.

use serde::Serialize;

use crate::geometry::{PixelRect, PixelSize};
use crate::three_a::SensorPoint;

/// Внутренние параметры камеры `[f_x, f_y, c_x, c_y, s]` в пикселях системы
/// `preCorrectionActiveArraySize` (`ACAMERA_LENS_INTRINSIC_CALIBRATION`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LensIntrinsics {
    /// Фокусное расстояние по горизонтали, пиксели.
    pub focal_length_x_px: f32,
    /// Фокусное расстояние по вертикали, пиксели.
    pub focal_length_y_px: f32,
    /// Оптическая ось по горизонтали, пиксели от левого края `preCorrectionActiveArraySize`.
    pub principal_point_x_px: f32,
    /// Оптическая ось по вертикали, пиксели от верхнего края `preCorrectionActiveArraySize`.
    pub principal_point_y_px: f32,
    /// Перекос плоскости сенсора (обычно 0).
    pub skew: f32,
}

/// Относительно чего задано положение камеры (`ACAMERA_LENS_POSE_REFERENCE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "code")]
pub enum PoseReference {
    /// Относительно основной камеры направления (у OnePlus 11 — у всех камер).
    PrimaryCamera,
    /// Относительно гироскопа.
    Gyroscope,
    /// Положение неизвестно, значения — умолчания по направлению камеры.
    Undefined,
    /// Автомобильная система координат.
    Automotive,
    /// Код, которого нет в NDK 30.
    Other(u8),
}

/// Положение камеры в корпусе (`ACAMERA_LENS_POSE_ROTATION` / `ACAMERA_LENS_POSE_TRANSLATION`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LensPose {
    pub reference: PoseReference,
    /// Кватернион `(x, y, z, w)` из системы координат датчиков Android в систему камеры.
    pub rotation_xyzw: [f32; 4],
    /// Положение оптического центра, метры.
    pub translation_m: [f32; 3],
}

/// Всё, что HAL сообщил о геометрии объектива. Поля — как пришли; правдоподобие проверяет
/// [`LensCalibration::optical_axis`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LensCalibration {
    /// `None` — ключа нет (он необязательный).
    pub intrinsics: Option<LensIntrinsics>,
    /// `None` — ключей положения нет.
    pub pose: Option<LensPose>,
    /// Система координат калибровки (`ACAMERA_SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE`).
    pub pre_correction_active_array: PixelRect,
}

/// Пороги правдоподобия калибровки. Значения — параметр конфига движка, а не константы здесь.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CalibrationPlausibility {
    /// Наибольшее допустимое смещение оптической оси от центра сенсора, доля длинной стороны.
    /// Ось дальше — заглушка или ошибка калибровки.
    pub max_principal_point_offset: f32,
}

/// Почему оптической оси нет.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum UncalibratedReason {
    /// HAL не отдал `intrinsicCalibration`.
    NotReported,
    /// Система координат калибровки вырождена (нулевой размер).
    DegenerateCoordinateSystem,
    /// Фокусное расстояние в пикселях не положительное или не число.
    NonPositiveFocalLength,
    /// Ось вне сенсора или в его углу (заглушка `0, 0`).
    PrincipalPointOutsideSensor,
    /// Ось дальше порога от центра: `offset` — доля длинной стороны.
    PrincipalPointTooFarFromCenter { offset: f32 },
    /// Калибровка HAL есть и правдоподобна, но профиль телефона объявил её неверной (замер на
    /// живых кадрах показал расхождение): ось не используется, кадрирование — по центру сенсора.
    DistrustedByProfile,
}

/// Где оптическая ось камеры в долях активной области (как [`SensorPoint`]).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum OpticalAxis {
    /// Ось по калибровке HAL.
    Calibrated { point: SensorPoint },
    /// Калибровки нет: кадрирование — по центру полезной области.
    Uncalibrated { reason: UncalibratedReason },
}

impl LensCalibration {
    /// Калибровки нет совсем (камера без ключей).
    pub fn not_reported(pre_correction_active_array: PixelRect) -> Self {
        Self {
            intrinsics: None,
            pose: None,
            pre_correction_active_array,
        }
    }

    /// Оптическая ось в долях активной области `active_array` или честная причина, почему её нет.
    ///
    /// Перевод `preCorrection → active`: вычитается смещение активной области внутри
    /// `preCorrectionActiveArraySize` (на OnePlus 11 обе области совпадают, смещение 0).
    /// Искажение объектива не учитывается: у всех камер телефона `lens.distortion` = 0.
    pub fn optical_axis(&self, active_array: PixelRect, plausibility: &CalibrationPlausibility) -> OpticalAxis {
        match self.check_axis(active_array, plausibility) {
            Ok(point) => OpticalAxis::Calibrated { point },
            Err(reason) => OpticalAxis::Uncalibrated { reason },
        }
    }

    fn check_axis(
        &self,
        active_array: PixelRect,
        plausibility: &CalibrationPlausibility,
    ) -> Result<SensorPoint, UncalibratedReason> {
        let intrinsics = self.intrinsics.ok_or(UncalibratedReason::NotReported)?;
        if is_degenerate(active_array.size) || is_degenerate(self.pre_correction_active_array.size) {
            return Err(UncalibratedReason::DegenerateCoordinateSystem);
        }
        let focal_ok = [intrinsics.focal_length_x_px, intrinsics.focal_length_y_px]
            .iter()
            .all(|focal| focal.is_finite() && *focal > 0.0);
        if !focal_ok {
            return Err(UncalibratedReason::NonPositiveFocalLength);
        }
        // Ось в пикселях активной области: калибровка задана от угла preCorrection.
        let shift_x = f64::from(active_array.left) - f64::from(self.pre_correction_active_array.left);
        let shift_y = f64::from(active_array.top) - f64::from(self.pre_correction_active_array.top);
        let axis_x = f64::from(intrinsics.principal_point_x_px) - shift_x;
        let axis_y = f64::from(intrinsics.principal_point_y_px) - shift_y;
        let (width, height) = (f64::from(active_array.size.width), f64::from(active_array.size.height));
        // Строго внутри: ось ровно на краю (в том числе заглушка 0, 0) физически невозможна.
        let inside = axis_x.is_finite() && axis_y.is_finite() && axis_x > 0.0 && axis_y > 0.0;
        if !inside || axis_x >= width || axis_y >= height {
            return Err(UncalibratedReason::PrincipalPointOutsideSensor);
        }
        let long_side = width.max(height);
        let offset = (axis_x - width / 2.0).hypot(axis_y - height / 2.0) / long_side;
        if offset > f64::from(plausibility.max_principal_point_offset) {
            return Err(UncalibratedReason::PrincipalPointTooFarFromCenter { offset: offset as f32 });
        }
        // Доли строго внутри (0, 1): SensorPoint::new не может отказать, но ошибку не глотаем.
        SensorPoint::new((axis_x / width) as f32, (axis_y / height) as f32)
            .map_err(|_| UncalibratedReason::PrincipalPointOutsideSensor)
    }
}

fn is_degenerate(size: PixelSize) -> bool {
    size.width == 0 || size.height == 0
}

#[cfg(test)]
#[path = "lens_calibration_tests.rs"]
mod tests;
