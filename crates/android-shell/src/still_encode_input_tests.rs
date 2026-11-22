use super::*;

fn settings() -> HeifEncodeSettings {
    HeifEncodeSettings {
        width: 4096,
        height: 3072,
        rotation_degrees: 90,
        quality: 95,
        exif_tiff: None,
    }
}

fn little_endian_tiff() -> Vec<u8> {
    vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0]
}

#[test]
fn valid_settings_are_converted_to_jni_types() {
    let request = prepare_heif_request(&settings()).unwrap();
    assert_eq!(
        request,
        HeifRequest {
            width: 4096,
            height: 3072,
            rotation_degrees: 90,
            quality: 95,
            exif_block: None,
        }
    );
}

#[test]
fn exif_block_gets_exif_header_before_tiff() {
    let tiff = little_endian_tiff();
    let request = prepare_heif_request(&HeifEncodeSettings {
        exif_tiff: Some(tiff.clone()),
        ..settings()
    })
    .unwrap();
    let block = request.exif_block.unwrap();
    assert_eq!(&block[..6], b"Exif\0\0");
    assert_eq!(&block[6..], tiff.as_slice());
}

#[test]
fn big_endian_tiff_is_accepted() {
    let block = exif_block_from_tiff(&[b'M', b'M', 0, 42, 0, 0, 0, 8]).unwrap();
    assert_eq!(block.len(), 6 + 8);
}

#[test]
fn exif_with_ready_header_is_rejected_not_doubled() {
    let mut already_with_header = b"Exif\0\0".to_vec();
    already_with_header.extend_from_slice(&little_endian_tiff());
    assert!(matches!(
        exif_block_from_tiff(&already_with_header),
        Err(EncodeError::InvalidInput(_))
    ));
}

#[test]
fn empty_exif_is_rejected() {
    assert!(matches!(exif_block_from_tiff(&[]), Err(EncodeError::InvalidInput(_))));
}

#[test]
fn zero_or_oversized_dimensions_are_rejected() {
    for (width, height) in [(0, 10), (10, 0), (u32::MAX, 10), (10, u32::MAX)] {
        let result = prepare_heif_request(&HeifEncodeSettings {
            width,
            height,
            ..settings()
        });
        assert!(matches!(result, Err(EncodeError::InvalidInput(_))), "{width}x{height}");
    }
}

#[test]
fn rotation_other_than_right_angles_is_rejected() {
    for rotation_degrees in [45, 91, 360] {
        let result = prepare_heif_request(&HeifEncodeSettings {
            rotation_degrees,
            ..settings()
        });
        assert!(
            matches!(result, Err(EncodeError::InvalidInput(_))),
            "{rotation_degrees}"
        );
    }
    for rotation_degrees in [0, 90, 180, 270] {
        assert!(
            prepare_heif_request(&HeifEncodeSettings {
                rotation_degrees,
                ..settings()
            })
            .is_ok()
        );
    }
}

#[test]
fn heif_quality_above_100_is_rejected() {
    let result = prepare_heif_request(&HeifEncodeSettings {
        quality: 101,
        ..settings()
    });
    assert!(matches!(result, Err(EncodeError::InvalidInput(_))));
}

#[test]
fn jpeg_request_requires_exact_rgba_length() {
    let ok = prepare_jpeg_request(2 * 3 * 4, 2, 3, 90).unwrap();
    assert_eq!(
        ok,
        JpegRequest {
            width: 2,
            height: 3,
            quality: 90
        }
    );
    // Буфер на байт короче и на байт длиннее — оба отвергаются.
    assert!(prepare_jpeg_request(2 * 3 * 4 - 1, 2, 3, 90).is_err());
    assert!(prepare_jpeg_request(2 * 3 * 4 + 1, 2, 3, 90).is_err());
}

#[test]
fn jpeg_request_rejects_zero_size_and_bad_quality() {
    assert!(prepare_jpeg_request(0, 0, 3, 90).is_err());
    assert!(prepare_jpeg_request(0, 3, 0, 90).is_err());
    assert!(prepare_jpeg_request(2 * 3 * 4, 2, 3, 101).is_err());
}

#[test]
fn jpeg_size_overflow_is_invalid_input_not_panic() {
    let result = prepare_jpeg_request(16, i32::MAX as u32, i32::MAX as u32, 90);
    assert!(matches!(result, Err(EncodeError::InvalidInput(_))));
}

#[test]
fn stop_timeout_is_never_zero_because_zero_means_forever() {
    assert_eq!(stop_timeout_millis(Duration::ZERO), 1);
    assert_eq!(stop_timeout_millis(Duration::from_micros(300)), 1);
    assert_eq!(stop_timeout_millis(Duration::from_secs(30)), 30_000);
    assert_eq!(stop_timeout_millis(Duration::MAX), i64::MAX);
}

#[test]
fn finish_codes_map_to_distinct_outcomes() {
    let timeout = Duration::from_secs(5);
    assert_eq!(heif_finish_result(FINISH_DONE, timeout), Ok(()));
    assert_eq!(
        heif_finish_result(FINISH_TIMED_OUT, timeout),
        Err(EncodeError::TimedOut(timeout))
    );
    assert!(matches!(
        heif_finish_result(7, timeout),
        Err(EncodeError::Platform(text)) if text.contains('7')
    ));
}
