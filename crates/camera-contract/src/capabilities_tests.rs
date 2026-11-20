use super::*;
use crate::fixtures;

#[test]
fn largest_raw_stream_picks_requested_format_by_pixel_count() {
    let mut capabilities = fixtures::main_wide_camera().capabilities;
    capabilities.raw_streams.push(RawStreamOption {
        format: RawFormat::Raw16,
        size: PixelSize::new(2048, 1536),
        mosaic: capabilities.raw_streams[0].mosaic,
        min_frame_duration: FrameDuration::from_nanos(33_333_333),
    });
    let raw16 = capabilities.largest_raw_stream(RawFormat::Raw16).expect("RAW16 есть");
    assert_eq!(raw16.size, PixelSize::new(4096, 3072));
    assert_eq!(raw16.format, RawFormat::Raw16);

    capabilities
        .raw_streams
        .retain(|option| option.format == RawFormat::Raw16);
    assert!(capabilities.largest_raw_stream(RawFormat::Raw10).is_none());
}

#[test]
fn field_of_view_uses_long_sensor_side_and_shortest_focal_length() {
    // Сенсор 36×24 мм с объективом 18 мм: 2·atan(18/18) = 90°.
    let optics = Optics {
        apertures_f_number: vec![2.0],
        focal_lengths_mm: vec![50.0, 18.0],
        sensor_physical_size: PhysicalSizeMm {
            width_mm: 24.0,
            height_mm: 36.0,
        },
        calibration: LensCalibration::not_reported(PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(6000, 4000),
        }),
        max_digital_zoom: None,
    };
    let fov = optics.widest_field_of_view_degrees().expect("угол есть");
    assert!((fov - 90.0).abs() < 1e-9, "угол {fov}");
}

#[test]
fn field_of_view_is_absent_for_broken_optics() {
    let mut optics = fixtures::main_wide_camera().capabilities.optics;
    optics.focal_lengths_mm = vec![0.0, f32::NAN];
    assert_eq!(optics.widest_field_of_view_degrees(), None);

    let mut optics = fixtures::main_wide_camera().capabilities.optics;
    optics.sensor_physical_size.width_mm = 0.0;
    optics.sensor_physical_size.height_mm = 0.0;
    assert_eq!(optics.widest_field_of_view_degrees(), None);
}

#[test]
fn frame_duration_converts_to_rate_and_zero_means_unbounded() {
    let rate = FrameDuration::from_nanos(16_666_667)
        .frames_per_second()
        .expect("частота есть");
    assert!((rate - 60.0).abs() < 1e-3, "частота {rate}");
    assert_eq!(FrameDuration::from_nanos(0).frames_per_second(), None);
}

#[test]
fn rational_with_zero_denominator_has_no_value() {
    assert_eq!(
        Rational {
            numerator: 3,
            denominator: 4
        }
        .to_f64(),
        Some(0.75)
    );
    assert_eq!(
        Rational {
            numerator: 3,
            denominator: 0
        }
        .to_f64(),
        None
    );
}

#[test]
fn reference_illuminant_round_trips_exif_codes() {
    for code in 0..=255u8 {
        assert_eq!(ReferenceIlluminant::from_exif_code(code).exif_code(), code);
    }
    assert_eq!(ReferenceIlluminant::from_exif_code(21), ReferenceIlluminant::D65);
}
