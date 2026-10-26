use camera_contract::capabilities::Rational;
use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera};
use camera_contract::raw_format::MosaicKind;

use super::*;
use crate::entry::EntryKind;
use crate::fake_metadata::{FakeMetadata, main_camera_characteristics};

fn parse_main(source: &FakeMetadata) -> Result<ParsedCharacteristics, CharacteristicsError> {
    parse_characteristics(source, NativeMosaic::Known(MosaicKind::QuadBayer))
}

#[test]
fn main_camera_numbers_from_session_00_become_the_reference_capabilities_without_notes() {
    let parsed = parse_main(&main_camera_characteristics()).expect("полные характеристики разбираются");
    // Сверка с образцом: RAW 4096×3072 RAW10+RAW16, 33,3 мс, BGGR, black 64 / white 1023,
    // коррекция виньетирования применена вендором, ручной сенсор 100–6400.
    assert_eq!(parsed.capabilities, main_wide_camera().capabilities);
    assert_eq!(
        parsed.notes,
        Vec::new(),
        "полный набор ключей не должен порождать заметок"
    );
}

#[test]
fn output_only_raw_formats_are_taken_and_jpeg_and_input_configurations_are_skipped() {
    let parsed = parse_main(&main_camera_characteristics()).unwrap();
    let streams = &parsed.capabilities.raw_streams;
    assert_eq!(streams.len(), 2, "RAW10 и RAW16; JPEG и входная конфигурация лишние");
    assert!(streams.iter().all(|option| option.size == PixelSize::new(4096, 3072)));
    assert!(
        streams
            .iter()
            .all(|option| option.min_frame_duration.as_nanos() == 33_333_333)
    );
    assert!(streams.iter().all(|option| option.mosaic.kind == MosaicKind::Bayer));
}

#[test]
fn active_array_is_left_top_width_height_not_right_bottom() {
    let source = main_camera_characteristics().int32s(tags::ACTIVE_ARRAY_SIZE, &[8, 16, 4080, 3040]);
    let rect = parse_main(&source).unwrap().capabilities.active_array;
    assert_eq!(rect.left, 8);
    assert_eq!(rect.top, 16);
    assert_eq!(rect.size, PixelSize::new(4080, 3040));
}

#[test]
fn front_camera_without_focus_range_is_fixed_focus_and_without_notes_for_focus_keys() {
    let source = main_camera_characteristics()
        .bytes(tags::LENS_FACING, &[tags::LENS_FACING_FRONT])
        .floats(tags::MIN_FOCUS_DISTANCE, &[0.0])
        .without(tags::HYPERFOCAL_DISTANCE)
        .without(tags::FOCUS_DISTANCE_CALIBRATION);
    let parsed = parse_main(&source).unwrap();
    assert_eq!(parsed.capabilities.focus, FocusCapability::Fixed);
    assert_eq!(parsed.capabilities.facing, LensFacing::Front);
    assert_eq!(front_camera().capabilities.focus, FocusCapability::Fixed);
    assert_eq!(parsed.notes, Vec::new());
}

#[test]
fn logical_camera_physical_ids_are_split_by_nul_and_hidden_ids_survive() {
    let source = main_camera_characteristics()
        .bytes(
            tags::AVAILABLE_CAPABILITIES,
            &[
                tags::CAPABILITY_BACKWARD_COMPATIBLE,
                tags::CAPABILITY_RAW,
                tags::CAPABILITY_LOGICAL_MULTI_CAMERA,
            ],
        )
        .bytes(tags::LOGICAL_PHYSICAL_IDS, b"2\x003\x004\x00");
    let parsed = parse_main(&source).unwrap();
    assert_eq!(
        parsed.capabilities.topology,
        CameraTopology::Logical {
            physical_ids: vec![camera_id("2"), camera_id("3"), camera_id("4")]
        }
    );
}

#[test]
fn logical_camera_without_physical_ids_is_logical_with_empty_list_and_a_note() {
    let source = main_camera_characteristics().bytes(
        tags::AVAILABLE_CAPABILITIES,
        &[tags::CAPABILITY_RAW, tags::CAPABILITY_LOGICAL_MULTI_CAMERA],
    );
    let parsed = parse_main(&source).unwrap();
    assert_eq!(
        parsed.capabilities.topology,
        CameraTopology::Logical {
            physical_ids: Vec::new()
        }
    );
    assert!(parsed.notes.contains(&CharacteristicNote {
        tag: "ACAMERA_LOGICAL_MULTI_CAMERA_PHYSICAL_IDS",
        issue: CharacteristicIssue::Absent
    }));
}

#[test]
fn unknown_capability_code_is_kept_not_dropped() {
    let source = main_camera_characteristics().bytes(tags::AVAILABLE_CAPABILITIES, &[tags::CAPABILITY_RAW, 19, 14]);
    let capabilities = parse_main(&source).unwrap().capabilities.request_capabilities;
    assert_eq!(
        capabilities,
        vec![
            RequestCapability::Raw,
            RequestCapability::Other(19),
            RequestCapability::SystemCamera
        ]
    );
}

#[test]
fn camera_without_raw_capability_has_no_raw_streams_and_needs_no_cfa() {
    let source = main_camera_characteristics()
        .bytes(tags::AVAILABLE_CAPABILITIES, &[tags::CAPABILITY_BACKWARD_COMPATIBLE])
        .without(tags::COLOR_FILTER_ARRANGEMENT)
        .without(tags::WHITE_LEVEL);
    let capabilities = parse_main(&source).unwrap().capabilities;
    assert_eq!(capabilities.raw, RawSupport::NotSupported);
    assert!(capabilities.raw_streams.is_empty());
    assert_eq!(capabilities.manual_sensor, ManualSensorRanges::NotSupported);
}

#[test]
fn raw_camera_without_white_level_is_an_error_not_a_zero_level() {
    let source = main_camera_characteristics().without(tags::WHITE_LEVEL);
    assert_eq!(
        parse_main(&source).unwrap_err(),
        CharacteristicsError::MissingRequired { tag: tags::WHITE_LEVEL }
    );
    let no_cfa = main_camera_characteristics().without(tags::COLOR_FILTER_ARRANGEMENT);
    assert_eq!(
        parse_main(&no_cfa).unwrap_err(),
        CharacteristicsError::MissingRequired {
            tag: tags::COLOR_FILTER_ARRANGEMENT
        }
    );
}

#[test]
fn missing_or_broken_required_keys_give_distinct_errors() {
    assert_eq!(
        parse_main(&main_camera_characteristics().without(tags::LENS_FACING)).unwrap_err(),
        CharacteristicsError::MissingRequired { tag: tags::LENS_FACING }
    );
    assert_eq!(
        parse_main(&main_camera_characteristics().bytes(tags::HARDWARE_LEVEL, &[9])).unwrap_err(),
        CharacteristicsError::UnknownValue {
            tag: tags::HARDWARE_LEVEL,
            value: 9
        }
    );
    // Тип записи не тот: размер массива пришёл как float.
    assert_eq!(
        parse_main(&main_camera_characteristics().floats(tags::PIXEL_ARRAY_SIZE, &[4096.0, 3072.0])).unwrap_err(),
        CharacteristicsError::Malformed {
            tag: tags::PIXEL_ARRAY_SIZE,
            problem: EntryProblem::WrongType {
                expected: EntryKind::Int32,
                actual: EntryKind::Float
            }
        }
    );
    // Размер из одного элемента.
    assert_eq!(
        parse_main(&main_camera_characteristics().int32s(tags::PIXEL_ARRAY_SIZE, &[4096])).unwrap_err(),
        CharacteristicsError::Malformed {
            tag: tags::PIXEL_ARRAY_SIZE,
            problem: EntryProblem::TooFewElements { expected: 2, actual: 1 }
        }
    );
    // Отрицательный размер.
    assert_eq!(
        parse_main(&main_camera_characteristics().int32s(tags::PIXEL_ARRAY_SIZE, &[-1, 3072])).unwrap_err(),
        CharacteristicsError::Malformed {
            tag: tags::PIXEL_ARRAY_SIZE,
            problem: EntryProblem::OutOfRange
        }
    );
    // Чтение не удалось.
    assert_eq!(
        parse_main(&main_camera_characteristics().unreadable(tags::LENS_FACING, -10001)).unwrap_err(),
        CharacteristicsError::Malformed {
            tag: tags::LENS_FACING,
            problem: EntryProblem::Unreadable { status: -10001 }
        }
    );
}

#[test]
fn missing_min_frame_durations_become_zero_which_means_not_limited_and_leave_a_note() {
    let parsed = parse_main(&main_camera_characteristics().without(tags::MIN_FRAME_DURATIONS)).unwrap();
    assert!(
        parsed
            .capabilities
            .raw_streams
            .iter()
            .all(|option| option.min_frame_duration.as_nanos() == 0)
    );
    assert!(
        parsed
            .capabilities
            .raw_streams
            .iter()
            .all(|option| option.min_frame_duration.frames_per_second().is_none())
    );
    let tags_with_notes: Vec<_> = parsed.notes.iter().map(|note| note.tag).collect();
    assert_eq!(tags_with_notes, vec!["ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS"; 2]);
}

#[test]
fn manual_sensor_declared_without_ranges_is_not_supported_with_notes() {
    let source = main_camera_characteristics()
        .without(tags::SENSITIVITY_RANGE)
        .without(tags::MAX_FRAME_DURATION);
    let parsed = parse_main(&source).unwrap();
    assert_eq!(parsed.capabilities.manual_sensor, ManualSensorRanges::NotSupported);
    let noted: Vec<_> = parsed.notes.iter().map(|note| note.tag).collect();
    assert!(noted.contains(&"ACAMERA_SENSOR_INFO_SENSITIVITY_RANGE"));
    assert!(noted.contains(&"ACAMERA_SENSOR_INFO_MAX_FRAME_DURATION"));
}

#[test]
fn manual_sensor_ranges_carry_hal_numbers() {
    let parsed = parse_main(&main_camera_characteristics()).unwrap();
    let ManualSensorRanges::Supported {
        iso,
        exposure_time,
        max_frame_duration,
    } = parsed.capabilities.manual_sensor
    else {
        panic!("ручной сенсор объявлен");
    };
    assert_eq!((iso.min, iso.max), (100, 6400));
    assert_eq!(exposure_time.shortest, Duration::from_micros(10));
    assert_eq!(exposure_time.longest, Duration::from_secs(32));
    assert_eq!(max_frame_duration.as_nanos(), 32_000_000_000);
}

#[test]
fn absent_hyperfocal_distance_stays_unknown_and_calibration_defaults_to_uncalibrated() {
    let source = main_camera_characteristics()
        .without(tags::HYPERFOCAL_DISTANCE)
        .without(tags::FOCUS_DISTANCE_CALIBRATION);
    let parsed = parse_main(&source).unwrap();
    assert_eq!(
        parsed.capabilities.focus,
        FocusCapability::Adjustable {
            min_focus_distance_diopters: 10.0,
            hyperfocal_distance_diopters: None,
            calibration: FocusCalibration::Uncalibrated
        }
    );
    assert!(
        parsed
            .notes
            .iter()
            .any(|note| note.tag == "ACAMERA_LENS_INFO_HYPERFOCAL_DISTANCE"
                && matches!(note.issue, CharacteristicIssue::Absent))
    );
}

#[test]
fn color_calibration_set_without_matrices_is_skipped_and_missing_transform_is_identity() {
    let source = main_camera_characteristics()
        .without(tags::FORWARD_MATRIX2)
        .without(tags::CALIBRATION_TRANSFORM1);
    let parsed = parse_main(&source).unwrap();
    let RawSupport::Supported(info) = parsed.capabilities.raw else {
        panic!("RAW поддерживается");
    };
    assert_eq!(info.color_calibration.len(), 1, "набор 2 без прямой матрицы пропущен");
    let identity = Rational {
        numerator: 1,
        denominator: 1,
    };
    assert_eq!(info.color_calibration[0].calibration_transform.0[0], identity);
    assert_eq!(info.color_calibration[0].illuminant, ReferenceIlluminant::D65);
    let issues: Vec<_> = parsed.notes.iter().map(|note| note.tag).collect();
    assert!(issues.contains(&"ACAMERA_SENSOR_FORWARD_MATRIX2"));
    assert!(issues.contains(&"ACAMERA_SENSOR_CALIBRATION_TRANSFORM1"));
}

#[test]
fn lens_shading_states_are_distinguished() {
    let shading = |source: FakeMetadata| parse_main(&source).unwrap().capabilities.vendor_processing.lens_shading;
    assert_eq!(shading(main_camera_characteristics()), LensShading::AppliedByVendor);
    assert_eq!(
        shading(main_camera_characteristics().bytes(tags::LENS_SHADING_APPLIED, &[tags::LENS_SHADING_APPLIED_FALSE])),
        LensShading::NotApplied
    );
    assert_eq!(
        shading(main_camera_characteristics().without(tags::LENS_SHADING_APPLIED)),
        LensShading::NotReported
    );
}

#[test]
fn native_mosaic_comes_from_the_caller_and_defaults_do_not_leak_in() {
    let parsed = parse_characteristics(&main_camera_characteristics(), NativeMosaic::Unknown).unwrap();
    let RawSupport::Supported(info) = parsed.capabilities.raw else {
        panic!("RAW поддерживается");
    };
    assert_eq!(info.native_mosaic, NativeMosaic::Unknown);
}

#[test]
fn camera_id_helper_matches_physical_id_chunks() {
    assert_eq!(
        split_physical_ids(b"2\x00\x003\x00"),
        vec![camera_id("2"), camera_id("3")]
    );
    assert_eq!(split_physical_ids(b""), Vec::<CameraId>::new());
}
