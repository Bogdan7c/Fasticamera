use camera_contract::geometry::PixelSize;

use super::{SENSOR_CLIP_LEVEL, WhiteBalanceGains, apply_white_balance};
use crate::cfa::{BayerPattern, RgbChannel};
use crate::color::dng::CameraNeutral;
use crate::image::{ShadingCorrectedMosaic, mosaic_stage};

/// Нейтраль основной камеры OnePlus 11 из паспорта C7 «лампа».
fn lamp_neutral() -> CameraNeutral {
    CameraNeutral::new([0.515625, 1.0, 0.499_023_437_5]).unwrap()
}

/// Мозаика BGGR 2×2: значения по цветам CFA.
fn bggr_quad(red: f32, green: f32, blue: f32) -> ShadingCorrectedMosaic {
    ShadingCorrectedMosaic::from_samples(
        PixelSize::new(2, 2),
        BayerPattern::BGGR,
        vec![blue, green, green, red],
        mosaic_stage::ShadingCorrected,
    )
    .unwrap()
}

#[test]
fn gains_are_inverse_neutral_with_smallest_gain_one() {
    let gains = WhiteBalanceGains::from_camera_neutral(lamp_neutral());
    assert_eq!(gains.gain(RgbChannel::Green), 1.0);
    assert!((gains.gain(RgbChannel::Red) - 1.0 / 0.515625).abs() < 1e-6);
    assert!((gains.gain(RgbChannel::Blue) - 1.0 / 0.499_023_44).abs() < 1e-6);
    // Совпадает с усилениями AWB вендора из того же паспорта (1,938 / 1 / 2,004).
    assert!((gains.gain(RgbChannel::Red) - 1.9384766).abs() < 2e-3);
    assert!((gains.gain(RgbChannel::Blue) - 2.0039062).abs() < 2e-3);
}

#[test]
fn smallest_gain_is_one_even_when_green_is_not_brightest() {
    let gains = WhiteBalanceGains::from_camera_neutral(CameraNeutral::new([2.0, 1.0, 0.5]).unwrap());
    assert_eq!(gains.as_array(), [1.0, 2.0, 4.0]);
}

#[test]
fn gray_object_becomes_equal_rgb_after_balance() {
    let neutral = lamp_neutral().values();
    // Серый объект яркости 0,3 в камере даёт RGB = 0,3 · N.
    let mosaic = bggr_quad(0.3 * neutral[0] as f32, 0.3, 0.3 * neutral[2] as f32);
    let balanced = apply_white_balance(&mosaic, &WhiteBalanceGains::from_camera_neutral(lamp_neutral()));
    for &sample in balanced.samples() {
        assert!((sample - 0.3).abs() < 1e-6, "{:?}", balanced.samples());
    }
}

#[test]
fn saturated_sensor_pixel_stays_white_not_pink() {
    // Все каналы насыщены в сенсоре (1,0): без обрезки R и B стали бы ~2 при G = 1.
    let balanced = apply_white_balance(
        &bggr_quad(1.0, 1.0, 1.0),
        &WhiteBalanceGains::from_camera_neutral(lamp_neutral()),
    );
    assert!(
        balanced.samples().iter().all(|&sample| sample == SENSOR_CLIP_LEVEL),
        "{:?}",
        balanced.samples()
    );
}

#[test]
fn balance_keeps_pattern_and_size() {
    let balanced = apply_white_balance(
        &bggr_quad(0.1, 0.2, 0.3),
        &WhiteBalanceGains::from_camera_neutral(lamp_neutral()),
    );
    assert_eq!(balanced.pattern(), BayerPattern::BGGR);
    assert_eq!(balanced.size(), PixelSize::new(2, 2));
}
