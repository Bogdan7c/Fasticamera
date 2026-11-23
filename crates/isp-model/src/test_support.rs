//! Общие данные модульных тестов: паспорт и калибровка основной камеры OnePlus 11
//! (паспорт C7 «лампа», 00 C7; матрицы — как их отдаёт HAL).

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Matrix3x3, Rational, ReferenceIlluminant, SensorLevels, VendorProcessingFlags,
};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::lens_shading::{
    GAIN_CHANNELS, LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid, ValidAreaOnSensor,
};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use std::sync::Arc;

use crate::color::dng::CameraNeutral;

/// Матрица из десятичных значений: знаменатель 2^20 точно хранит дроби HAL (/128, /10000 — с ошибкой < 1e-6).
fn rational_matrix(values: [f64; 9]) -> Matrix3x3 {
    const DENOMINATOR: i32 = 1 << 20;
    Matrix3x3(values.map(|value| Rational {
        numerator: (value * f64::from(DENOMINATOR)).round() as i32,
        denominator: DENOMINATOR,
    }))
}

/// ColorMatrix1 (D65), основная камера.
pub(crate) const MAIN_COLOR_MATRIX_D65: [f64; 9] = [
    0.9848, -0.3684, -0.1011, -0.2682, 1.1295, 0.1152, 0.0588, 0.0403, 0.5348,
];
/// ColorMatrix2 (StdA).
pub(crate) const MAIN_COLOR_MATRIX_A: [f64; 9] =
    [1.9084, -1.3277, 0.1113, -0.2221, 1.1645, 0.2236, 0.141, -0.0884, 0.8508];
/// ForwardMatrix1 = ForwardMatrix2 (HAL отдаёт одинаковые).
pub(crate) const MAIN_FORWARD_MATRIX: [f64; 9] = [
    0.4375, 0.3828125, 0.140625, 0.21875, 0.71875, 0.0625, 0.015625, 0.09375, 0.7109375,
];
/// Нейтраль кадра «лампа» (`SENSOR_NEUTRAL_COLOR_POINT`).
pub(crate) const LAMP_NEUTRAL: [f64; 3] = [0.515625, 1.0, 0.4990234375];

pub(crate) fn main_camera_calibration() -> Vec<ColorCalibrationSet> {
    vec![
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::D65,
            color_matrix: rational_matrix(MAIN_COLOR_MATRIX_D65),
            forward_matrix: rational_matrix(MAIN_FORWARD_MATRIX),
            calibration_transform: rational_matrix([1.0078125, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0078125]),
        },
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::StandardA,
            color_matrix: rational_matrix(MAIN_COLOR_MATRIX_A),
            forward_matrix: rational_matrix(MAIN_FORWARD_MATRIX),
            calibration_transform: rational_matrix([0.9921875, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.9765625]),
        },
    ]
}

pub(crate) fn lamp_neutral() -> CameraNeutral {
    match CameraNeutral::new(LAMP_NEUTRAL) {
        Ok(neutral) => neutral,
        Err(error) => panic!("нейтраль C7 валидна: {error}"),
    }
}

/// Паспорт потока основной камеры 4:3@60 RAW10 (как в сессии 05).
pub(crate) fn main_camera_passport() -> StreamPassport {
    let buffer_size = PixelSize::new(4096, 3072);
    StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw10,
        buffer_size,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: buffer_size,
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(buffer_size),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: match TargetFrameRate::new(60) {
            Ok(rate) => rate,
            Err(error) => panic!("60 к/с допустимо: {error}"),
        },
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: main_camera_calibration(),
    }
}

/// Активная область основной камеры OnePlus 11.
pub(crate) const MAIN_ACTIVE_ARRAY: PixelSize = PixelSize::new(4096, 3072);

/// Углы поля усилений по каналам `[R, G_even, G_odd, B]`, в порядке «левый верх, правый верх,
/// левый низ, правый низ». Все углы разные и в разных каналах — перестановка осей, каналов или углов
/// сразу ломает тест (углы 2,2–3 EV темнее центра, как у настоящего телефона).
pub(crate) const ASYMMETRIC_CORNER_GAINS: [[f64; 4]; GAIN_CHANNELS] = [
    [3.2, 1.6, 2.4, 4.5],
    [2.9, 1.5, 2.2, 4.0],
    [3.0, 1.55, 2.25, 4.2],
    [3.6, 1.8, 2.7, 5.0],
];

/// Билинейное поле усилений канала `channel` в пикселе `(x, y)` активной области (в f64): независимый
/// от кода ISP оракул для геометрических тестов — билинейное поле пересчёт сетки воспроизводит точно.
pub(crate) fn corner_field_gain(channel: usize, x: f64, y: f64, active: PixelSize) -> f64 {
    let [top_left, top_right, bottom_left, bottom_right] = ASYMMETRIC_CORNER_GAINS[channel];
    let u = x / f64::from(active.width - 1);
    let v = y / f64::from(active.height - 1);
    (top_left * (1.0 - u) + top_right * u) * (1.0 - v) + (bottom_left * (1.0 - u) + bottom_right * u) * v
}

/// Калибровка камеры «2» на сетке `columns × rows`: узлы лежат на билинейном поле углов.
pub(crate) fn asymmetric_calibration(columns: u32, rows: u32) -> LensShadingCalibration {
    let mut points = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let x = f64::from(MAIN_ACTIVE_ARRAY.width - 1) * f64::from(column) / f64::from(columns - 1);
            let y = f64::from(MAIN_ACTIVE_ARRAY.height - 1) * f64::from(row) / f64::from(rows - 1);
            let mut gains = [1.0_f32; GAIN_CHANNELS];
            for (channel, gain) in gains.iter_mut().enumerate() {
                *gain = corner_field_gain(channel, x, y, MAIN_ACTIVE_ARRAY) as f32;
            }
            points.push(gains);
        }
    }
    LensShadingCalibration {
        camera: camera_id("2"),
        active_array: MAIN_ACTIVE_ARRAY,
        grid: match LensShadingGainGrid::new(columns, rows, points) {
            Ok(grid) => grid,
            Err(error) => panic!("синтетическая сетка валидна: {error}"),
        },
    }
}

/// Паспорт основной камеры с нашей калибровкой: буфер `buffer_size`, полезная область — прямоугольник
/// `area` активной области (буфер меньше области — биннинг).
pub(crate) fn calibrated_passport(buffer_size: PixelSize, area: PixelRect) -> StreamPassport {
    let mut passport = main_camera_passport();
    passport.buffer_size = buffer_size;
    passport.valid_area = PixelRect {
        left: 0,
        top: 0,
        size: buffer_size,
    };
    passport.sensor_area = match ValidAreaOnSensor::new(MAIN_ACTIVE_ARRAY, area) {
        Ok(area) => area,
        Err(error) => panic!("прямоугольник в активной области: {error}"),
    };
    passport.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(asymmetric_calibration(33, 25)));
    passport
}
