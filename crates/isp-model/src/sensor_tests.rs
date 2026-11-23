use camera_contract::capabilities::LensShading;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern};

use super::{SensorDescription, SensorDescriptionError};
use crate::cfa::{BayerPattern, CfaCell};
use crate::color::dng::ColorProfileError;
use crate::lens_shading::StreamMapError;
use crate::test_support::{
    MAIN_ACTIVE_ARRAY, calibrated_passport, corner_field_gain, main_camera_calibration, main_camera_passport,
};
use camera_contract::geometry::{PixelRect, PixelSize};

#[test]
fn oneplus_main_camera_passport_is_described_faithfully() {
    let sensor = SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap();
    assert_eq!(sensor.pattern(), BayerPattern::BGGR);
    assert_eq!(sensor.static_levels().black(CfaCell::of_pixel(1, 1)), 64.0);
    assert_eq!(sensor.static_levels().white(), 1023.0);
    assert_eq!(sensor.vendor_lens_shading(), LensShading::AppliedByVendor);
}

#[test]
fn dynamic_levels_override_static_ones_per_frame() {
    let sensor = SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap();
    let static_only = sensor.frame_levels(&CaptureColorHints::default()).unwrap();
    assert_eq!(
        static_only,
        sensor.static_levels(),
        "без динамических уровней — статические"
    );
    let hints = CaptureColorHints {
        dynamic_black_level: Some([63.5, 64.0, 64.5, 65.0]),
        dynamic_white_level: Some(1000),
        ..CaptureColorHints::default()
    };
    let dynamic = sensor.frame_levels(&hints).unwrap();
    assert_eq!(dynamic.black_pattern(), [63.5, 64.0, 64.5, 65.0]);
    assert_eq!(dynamic.white(), 1000.0);
}

#[test]
fn non_bayer_mosaic_is_rejected() {
    let mut passport = main_camera_passport();
    passport.mosaic = MosaicPattern {
        kind: MosaicKind::QuadBayer,
        layout: CfaLayout::Bggr,
    };
    assert_eq!(
        SensorDescription::from_stream_passport(&passport, &main_camera_calibration()),
        Err(SensorDescriptionError::UnsupportedMosaicKind {
            kind: MosaicKind::QuadBayer
        })
    );
}

#[test]
fn missing_color_calibration_is_an_error_not_identity() {
    assert_eq!(
        SensorDescription::from_stream_passport(&main_camera_passport(), &[]),
        Err(SensorDescriptionError::ColorProfile(ColorProfileError::NoCalibration))
    );
}

#[test]
fn calibrated_passport_yields_a_stream_map_and_uncalibrated_yields_none() {
    let uncalibrated =
        SensorDescription::from_stream_passport(&main_camera_passport(), &main_camera_calibration()).unwrap();
    assert!(uncalibrated.lens_shading_map().is_none());

    let area = PixelRect {
        left: 0,
        top: 384,
        size: PixelSize::new(4096, 2304),
    };
    let passport = calibrated_passport(PixelSize::new(64, 36), area);
    let sensor = SensorDescription::from_stream_passport(&passport, &main_camera_calibration()).unwrap();
    let map = sensor.lens_shading_map().expect("калибровка есть — карта построена");
    // Карта над полезной областью потока, в её координатах: сетка калибровки 33×25 сохранена,
    // а левый верхний узел — центр первого пикселя буфера: буфер 64×36 покрывает полосу 4096×2304
    // с масштабом 64, то есть активный пиксель (31,5; 384 + 31,5), а не угол сенсора.
    assert_eq!((map.columns(), map.rows()), (33, 25));
    let band_top_left = corner_field_gain(0, 31.5, 384.0 + 31.5, MAIN_ACTIVE_ARRAY);
    assert!((f64::from(map.points()[0][0]) - band_top_left).abs() < 1e-3 * band_top_left);
}

#[test]
fn calibration_that_cannot_be_mapped_is_a_sensor_error_not_silently_skipped() {
    // Верх полосы нечётный — чётность строк калибровки разошлась бы с CFA: честная ошибка.
    let area = PixelRect {
        left: 0,
        top: 385,
        size: PixelSize::new(4096, 2304),
    };
    let passport = calibrated_passport(PixelSize::new(64, 36), area);
    assert_eq!(
        SensorDescription::from_stream_passport(&passport, &main_camera_calibration()),
        Err(SensorDescriptionError::LensShading(StreamMapError::OddSensorTop {
            top: 385
        }))
    );
}
