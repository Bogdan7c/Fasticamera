use camera_contract::capabilities::LensShading;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern};

use super::{SensorDescription, SensorDescriptionError};
use crate::cfa::{BayerPattern, CfaCell};
use crate::color::dng::ColorProfileError;
use crate::test_support::{main_camera_calibration, main_camera_passport};

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
