//! Общие данные модульных тестов: паспорт и калибровка основной камеры OnePlus 11
//! (паспорт C7 «лампа», 00 C7; матрицы — как их отдаёт HAL).

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Matrix3x3, Rational, ReferenceIlluminant, SensorLevels, VendorProcessingFlags,
};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};

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
