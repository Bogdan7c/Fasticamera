//! Подтяжка экспозиции и веса яркости: проверяем результат на экране и на мозаике.

use camera_contract::frame_passport::CaptureColorHints;

use super::{ExposureGain, InvalidExposureGain};
use crate::color::dng::CameraNeutral;
use crate::color::temperature::Kelvin;
use crate::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use crate::demosaic::DemosaicMethod;
use crate::image::DisplayGamut;
use crate::params::{FrameInputs, IspParams, ProcessingControls};
use crate::sensor::SensorDescription;
use crate::test_support::{main_camera_calibration, main_camera_passport};
use crate::tone::{ToneCurve, ToneCurveSettings};

fn main_sensor() -> SensorDescription {
    SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap()
}

fn neutral_for(sensor: &SensorDescription, kelvin: f64) -> CameraNeutral {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .unwrap();
    sensor.color_profile().neutral_for_white(white).unwrap()
}

fn resolve_with(neutral: CameraNeutral, gain: ExposureGain) -> IspParams {
    let sensor = main_sensor();
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let mut controls =
        ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb).with_legacy_tone();
    controls.exposure_gain = gain;
    IspParams::resolve(&sensor, &frame, &controls).unwrap()
}

#[test]
fn double_gain_on_half_gray_shows_the_same_screen_level_as_unity_on_full_gray() {
    let sensor = main_sensor();
    let neutral = neutral_for(&sensor, 5000.0);
    let middle = ToneCurveSettings::default().scene_middle_gray;
    let boosted = resolve_with(neutral, ExposureGain::new(2.0).unwrap());
    let plain = resolve_with(neutral, ExposureGain::UNITY);
    let boosted_level = boosted.tone.map_channel(middle / 2.0);
    let plain_level = plain.tone.map_channel(middle);
    assert!(
        (boosted_level - plain_level).abs() < 1e-5,
        "{boosted_level} против {plain_level}"
    );
    // И это действительно целевой серый экрана, а не случайное совпадение двух ошибок.
    assert!((plain_level - ToneCurveSettings::default().display_middle_gray).abs() < 1e-5);
}

#[test]
fn sensor_white_stays_white_for_any_gain() {
    let neutral = neutral_for(&main_sensor(), 5000.0);
    for gain_value in [0.7_f32, 1.0, 1.4] {
        let params = resolve_with(neutral, ExposureGain::new(gain_value).unwrap());
        assert!(
            (params.tone.map_channel(1.0) - 1.0).abs() < 1e-6,
            "k = {gain_value}: белый сенсора перестал быть белым"
        );
        // А средний серый реально сдвинулся: подтяжка не превратилась в no-op.
        let shifted = params.tone.map_channel(0.075);
        let plain = resolve_with(neutral, ExposureGain::UNITY).tone.map_channel(0.075);
        if gain_value != 1.0 {
            assert!((shifted - plain).abs() > 0.005, "k = {gain_value}: серый не сдвинулся");
        }
    }
}

#[test]
fn unity_gain_is_bit_identical_to_the_curve_of_untouched_settings() {
    let neutral = neutral_for(&main_sensor(), 5000.0);
    let params = resolve_with(neutral, ExposureGain::UNITY);
    let untouched = ToneCurve::resolve(&ToneCurveSettings::default()).unwrap();
    assert_eq!(params.tone, untouched);
}

#[test]
fn gain_rejects_nan_zero_negative_and_overflow() {
    for bad in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
        assert!(matches!(
            ExposureGain::new(bad),
            Err(InvalidExposureGain::NotPositiveFinite { .. })
        ));
    }
    assert!(ExposureGain::from_ev(f32::NAN).is_err());
    assert!(ExposureGain::from_ev(200.0).is_err(), "2^200 не помещается в f32");
    let one_stop = ExposureGain::from_ev(1.0).unwrap();
    assert_eq!(one_stop.multiplier(), 2.0);
    assert_eq!(one_stop.ev(), 1.0);
    assert_eq!(ExposureGain::UNITY.ev(), 0.0);
}

#[test]
fn luminance_weights_give_the_balanced_gray_level_for_any_illuminant() {
    let sensor = main_sensor();
    for kelvin in [2800.0, 6504.0] {
        let neutral = neutral_for(&sensor, kelvin);
        let params = resolve_with(neutral, ExposureGain::UNITY);
        let weights = params.scene_luminance_weights();
        let gray_level = 0.2_f32;
        // Несбалансированная мозаика серого: t·N (G нормирован в 1).
        let [n_r, n_g, n_b] = neutral.values().map(|value| value as f32);
        let from_mosaic = weights.luminance(gray_level * n_r, gray_level * n_g, gray_level * n_b);
        // Сбалансированный серый имеет уровень t·max(N); тон видит ту же яркость на выходе матриц.
        let balanced = gray_level * n_r.max(n_g).max(n_b);
        assert!(
            (from_mosaic - balanced).abs() / balanced < 1e-4,
            "{kelvin} К: Y по мозаике {from_mosaic}, сбалансированный серый {balanced}"
        );
        // Сквозная сверка с реальными матрицами кадра: яркость на входе тона.
        let gains = params.white_balance.as_array();
        let balanced_rgb = [
            gray_level * n_r * gains[0],
            gray_level * n_g * gains[1],
            gray_level * n_b * gains[2],
        ];
        let scene = params
            .color
            .xyz_d50_to_scene()
            .apply(params.color.camera_to_xyz_d50().apply(balanced_rgb));
        assert!(
            (scene[1] - from_mosaic).abs() / from_mosaic < 1e-3,
            "{scene:?} против {from_mosaic}"
        );
    }
}
