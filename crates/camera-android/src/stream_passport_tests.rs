use camera_contract::capabilities::FpsRange;
use camera_contract::fixtures::{main_wide_camera, ultra_wide_camera};
use camera_contract::geometry::SensorOrientation;
use camera_contract::raw_format::{CfaLayout, RawFormat};
use camera_contract::stream::TargetFrameRate;

use camera_contract::lens_shading::{LensShadingCalibration, LensShadingGainGrid};

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
    let passport = build_stream_passport(
        &stream,
        &main_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
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
    let passport = build_stream_passport(
        &stream,
        &main_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
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
    let passport = build_stream_passport(&stream, &capabilities, vendor, &LensShadingCalibrations::none()).unwrap();
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
    // Калибровка цвета — та же, что в характеристиках, и непустая у основной камеры (07: ISP
    // видоискателя берёт матрицы только из паспорта).
    assert!(
        !raw.color_calibration.is_empty(),
        "у образца основной камеры есть DNG-матрицы"
    );
    assert_eq!(passport.color_calibration, raw.color_calibration);
}

#[test]
fn camera_without_raw_gives_an_error_instead_of_a_guessed_passport() {
    let mut capabilities = main_wide_camera().capabilities;
    capabilities.raw = RawSupport::NotSupported;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    assert_eq!(
        build_stream_passport(&stream, &capabilities, mechanism(), &LensShadingCalibrations::none()),
        Err(PassportError::RawNotSupported)
    );
}

#[test]
fn widescreen_strip_that_does_not_fit_the_buffer_is_an_error() {
    // Буфер 4000×2000: полоса 16:9 нужна высотой 2250 > 2000.
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(4000, 2000));
    assert_eq!(
        build_stream_passport(
            &stream,
            &ultra_wide_camera().capabilities,
            mechanism(),
            &LensShadingCalibrations::none()
        ),
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
    let passport = build_stream_passport(
        &stream,
        &ultra_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
    assert_eq!(passport.valid_area.size, PixelSize::new(4000, 2250));
}

fn calibration_for(camera: CameraId, active_array: PixelSize) -> LensShadingCalibration {
    LensShadingCalibration {
        camera,
        active_array,
        grid: LensShadingGainGrid::new(2, 2, vec![[1.0, 1.1, 1.2, 1.3]; 4]).unwrap(),
    }
}

#[test]
fn four_by_three_sits_on_the_whole_active_array() {
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let passport = build_stream_passport(
        &stream,
        &main_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
    assert_eq!(
        passport.sensor_area,
        ValidAreaOnSensor::whole_active_array(PixelSize::new(4096, 3072))
    );
}

#[test]
fn sixteen_by_nine_is_the_central_band_of_the_active_array() {
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(4096, 3072));
    let passport = build_stream_passport(
        &stream,
        &main_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
    // Полоса 4096×2304 по центру: (3072 − 2304) / 2 = 384 свободных строк сверху (чётное число).
    assert_eq!(
        passport.sensor_area.rect,
        PixelRect {
            left: 0,
            top: 384,
            size: PixelSize::new(4096, 2304)
        }
    );
    assert_eq!(passport.sensor_area.active_array, PixelSize::new(4096, 3072));
    // Полезная область буфера при этом по-прежнему сверху буфера.
    assert_eq!(passport.valid_area.top, 0);
}

#[test]
fn binned_buffer_keeps_the_band_in_active_array_pixels() {
    // Буфер вдвое меньше области (биннинг): полоса остаётся в пикселях активной области.
    let stream = stream_for(ReadoutAspect::SixteenByNine, PixelSize::new(2048, 1536));
    let passport = build_stream_passport(
        &stream,
        &main_wide_camera().capabilities,
        mechanism(),
        &LensShadingCalibrations::none(),
    )
    .unwrap();
    assert_eq!(passport.valid_area.size, PixelSize::new(2048, 1152));
    assert_eq!(passport.sensor_area.rect.size, PixelSize::new(4096, 2304));
}

#[test]
fn matching_calibration_is_attached_by_camera_and_active_array() {
    let capabilities = main_wide_camera().capabilities;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let set = LensShadingCalibrations::new(vec![calibration_for(
        stream.camera.clone(),
        capabilities.active_array.size,
    )]);
    let passport = build_stream_passport(&stream, &capabilities, mechanism(), &set).unwrap();
    let attached = passport
        .lens_shading
        .calibration()
        .expect("калибровка камеры приложена");
    assert_eq!(attached.camera, stream.camera);
    assert_eq!(attached.grid.columns(), 2);
}

#[test]
fn calibration_for_another_active_array_is_not_attached() {
    let capabilities = main_wide_camera().capabilities;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let wrong_array = PixelSize::new(4000, 3000);
    let set = LensShadingCalibrations::new(vec![calibration_for(stream.camera.clone(), wrong_array)]);
    let passport = build_stream_passport(&stream, &capabilities, mechanism(), &set).unwrap();
    assert_eq!(
        passport.lens_shading,
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::ActiveArrayMismatch {
            calibration: wrong_array,
            camera: capabilities.active_array.size,
        })
    );
}

#[test]
fn camera_missing_from_the_profile_or_device_outside_it_reports_why() {
    let capabilities = main_wide_camera().capabilities;
    let stream = stream_for(ReadoutAspect::FourByThree, PixelSize::new(4096, 3072));
    let other_camera = CameraId::new("9").unwrap();
    let set = LensShadingCalibrations::new(vec![calibration_for(other_camera, capabilities.active_array.size)]);
    let missing = build_stream_passport(&stream, &capabilities, mechanism(), &set).unwrap();
    assert_eq!(
        missing.lens_shading,
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::NoCalibrationForCamera)
    );
    let foreign = build_stream_passport(
        &stream,
        &capabilities,
        mechanism(),
        &LensShadingCalibrations::device_not_in_profile(),
    )
    .unwrap();
    assert_eq!(
        foreign.lens_shading,
        LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile)
    );
}
