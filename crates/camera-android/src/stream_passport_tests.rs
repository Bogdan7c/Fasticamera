use camera_contract::capabilities::FpsRange;
use camera_contract::fixtures::{main_wide_camera, ultra_wide_camera};
use camera_contract::geometry::SensorOrientation;
use camera_contract::raw_format::{CfaLayout, RawFormat};
use camera_contract::stream::TargetFrameRate;

use super::*;

fn stream_for(readout: ReadoutAspect, buffer: PixelSize) -> ResolvedStream {
    ResolvedStream {
        camera: main_wide_camera().id,
        raw_format: RawFormat::Raw10,
        buffer_size: buffer,
        readout,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с допустимо"),
    }
}

fn mechanism() -> FrameRateMechanism {
    FrameRateMechanism::PublicAeTargetRange {
        range: FpsRange { min: 60, max: 60 },
    }
}

#[test]
fn sixteen_by_nine_on_the_main_camera_takes_rows_0_to_2303_from_the_top() {
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(4096, 3072));
    let passport = build_stream_passport(&stream, &main_wide_camera().capabilities, mechanism()).unwrap();
    assert_eq!(
        passport.valid_area,
        PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(4096, 2304)
        }
    );
    // Буфер остаётся целым: ниже полезной области в нём нули, а не «другой размер».
    assert_eq!(passport.buffer_size, PixelSize::new(4096, 3072));
}

#[test]
fn four_by_three_uses_the_whole_buffer() {
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let passport = build_stream_passport(&stream, &main_wide_camera().capabilities, mechanism()).unwrap();
    assert_eq!(passport.valid_area.size, PixelSize::new(4096, 3072));
    assert_eq!((passport.valid_area.left, passport.valid_area.top), (0, 0));
}

#[test]
fn passport_carries_levels_mosaic_orientation_and_frame_rate_from_the_sources() {
    let capabilities = main_wide_camera().capabilities;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let vendor = FrameRateMechanism::VendorSensorFrameDurationOverride {
        frame_duration: TargetFrameRate::new(60).unwrap().frame_duration(),
    };
    let passport = build_stream_passport(&stream, &capabilities, vendor).unwrap();
    // Публичный RAW — биннинг до Bayer, раскладка из характеристик (BGGR у образца).
    assert_eq!(passport.mosaic.kind, MosaicKind::Bayer);
    assert_eq!(passport.mosaic.layout, CfaLayout::Bggr);
    let RawSupport::Supported(raw) = &capabilities.raw else {
        panic!("образец поддерживает RAW");
    };
    assert_eq!(passport.static_levels, raw.levels);
    assert_eq!(passport.vendor_processing, capabilities.vendor_processing);
    assert_eq!(passport.sensor_orientation, SensorOrientation::Degrees90);
    assert_eq!(passport.frame_rate.frames_per_second(), 60);
    assert_eq!(passport.frame_rate_mechanism, vendor);
    assert_eq!(passport.camera, stream.camera);
    assert_eq!(passport.raw_format, RawFormat::Raw10);
}

#[test]
fn camera_without_raw_gives_an_error_instead_of_a_guessed_passport() {
    let mut capabilities = main_wide_camera().capabilities;
    capabilities.raw = RawSupport::NotSupported;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    assert_eq!(
        build_stream_passport(&stream, &capabilities, mechanism()),
        Err(PassportError::RawNotSupported)
    );
}

#[test]
fn widescreen_strip_that_does_not_fit_the_buffer_is_an_error() {
    // Буфер 4000×2000: полоса 16:9 нужна высотой 2250 > 2000.
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(4000, 2000));
    assert_eq!(
        build_stream_passport(&stream, &ultra_wide_camera().capabilities, mechanism()),
        Err(PassportError::WidescreenDoesNotFit {
            buffer_width: 4000,
            buffer_height: 2000,
            required_height: 2250
        })
    );
}

#[test]
fn widescreen_on_the_ultra_wide_uses_the_same_rule_as_an_unmeasured_assumption() {
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(4000, 3000));
    let passport = build_stream_passport(&stream, &ultra_wide_camera().capabilities, mechanism()).unwrap();
    assert_eq!(passport.valid_area.size, PixelSize::new(4000, 2250));
}
