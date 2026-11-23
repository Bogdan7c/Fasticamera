//! Параметры обработки для модульных тестов: DNG-матрицы основной камеры OnePlus 11 (калибровка C7
//! из сессии 00, как в `isp-model/src/test_support.rs`) и нейтраль кадра «лампа».

#![allow(
    clippy::expect_used,
    reason = "помощник модульных тестов: битая константа должна ронять тест"
)]

use camera_contract::capabilities::{LensShading, ReferenceIlluminant};
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::{CameraNeutral, DngCalibration, DngColorProfile};
use isp_model::color::matrix::Matrix3;
use isp_model::color::temperature::calibration_illuminant_temperature;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::levels::NormalizationLevels;
use isp_model::look::LookControls;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::sensor::SensorDescription;
use isp_model::tone::base::ToneBase;

fn matrix(values: [f64; 9]) -> Matrix3 {
    Matrix3::from_rows([
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ])
}

fn calibration(illuminant: ReferenceIlluminant, color_matrix: [f64; 9], diagonal: [f64; 2]) -> DngCalibration {
    DngCalibration {
        temperature: calibration_illuminant_temperature(illuminant).expect("иллюминант калибровки"),
        color_matrix: matrix(color_matrix),
        forward_matrix: Some(matrix([
            0.4375, 0.3828125, 0.140625, 0.21875, 0.71875, 0.0625, 0.015625, 0.09375, 0.7109375,
        ])),
        camera_calibration: matrix([diagonal[0], 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, diagonal[1]]),
    }
}

/// Параметры `Quality` для раскладки `pattern` (уровни RAW10 64/1023, нейтраль «лампа», sRGB), прежняя основа тона.
pub(crate) fn quality_params(pattern: BayerPattern) -> IspParams {
    quality_params_with_base(pattern, ToneBase::Legacy)
}

/// Те же параметры с выбранной основой тона.
pub(crate) fn quality_params_with_base(pattern: BayerPattern, tone_base: ToneBase) -> IspParams {
    quality_params_with_look(pattern, tone_base, LookControls::default())
}

/// Те же параметры с выбранной основой тона и видом картинки.
pub(crate) fn quality_params_with_look(pattern: BayerPattern, tone_base: ToneBase, look: LookControls) -> IspParams {
    let profile = DngColorProfile::new(
        calibration(
            ReferenceIlluminant::D65,
            [
                0.9848, -0.3684, -0.1011, -0.2682, 1.1295, 0.1152, 0.0588, 0.0403, 0.5348,
            ],
            [1.0078125, 1.0078125],
        ),
        Some(calibration(
            ReferenceIlluminant::StandardA,
            [1.9084, -1.3277, 0.1113, -0.2221, 1.1645, 0.2236, 0.141, -0.0884, 0.8508],
            [0.9921875, 0.9765625],
        )),
        [1.0, 1.0, 1.0],
    )
    .expect("DNG-профиль");
    let levels = NormalizationLevels::new([64.0, 64.0, 64.0, 64.0], 1023.0).expect("уровни");
    let sensor = SensorDescription::new(pattern, levels, profile, LensShading::AppliedByVendor);
    let frame = FrameInputs { levels };
    let neutral = CameraNeutral::new([0.515625, 1.0, 0.4990234375]).expect("нейтраль");
    let controls = ProcessingControls {
        tone_base,
        look,
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb)
    };
    IspParams::resolve(&sensor, &frame, &controls).expect("параметры кадра")
}
