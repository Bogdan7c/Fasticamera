use camera_contract::fixtures::{front_camera, main_wide_camera};

use super::*;
use crate::characteristics::{CharacteristicIssue, CharacteristicNote, parse_characteristics};
use crate::entry::{EntryKind, EntryValue};
use crate::fake_metadata::{FakeMetadata, main_camera_characteristics};

/// Разбор только 3A, с заметками.
fn parse(source: &FakeMetadata) -> (ThreeACapabilities, Vec<CharacteristicNote>) {
    let mut notes = Notes::default();
    let capabilities = parse_three_a(source, &mut notes);
    (capabilities, notes.into_notes())
}

fn absent_note(tag: &'static str) -> CharacteristicNote {
    CharacteristicNote {
        tag,
        issue: CharacteristicIssue::Absent,
    }
}

#[test]
fn oneplus_back_camera_values_from_dumpsys_become_the_reference_capabilities() {
    let (capabilities, notes) = parse(&FakeMetadata::default().with_back_camera_three_a());
    assert_eq!(capabilities, main_wide_camera().capabilities.three_a);
    assert_eq!(notes, Vec::new(), "полный набор ключей не порождает заметок");
    // Явные числа из dumpsys: регионы [AE 1, AWB 0, AF 1], режимы AF 0..4.
    assert_eq!(
        (
            capabilities.max_regions.auto_exposure,
            capabilities.max_regions.auto_white_balance,
            capabilities.max_regions.auto_focus
        ),
        (1, 0, 1)
    );
    assert_eq!(capabilities.af_modes.len(), 5);
    assert!(capabilities.supports_af_mode(AfModeSupport::ContinuousPicture));
    assert!(capabilities.supports_af_mode(AfModeSupport::Off));
}

#[test]
fn front_camera_declares_only_af_off_and_no_af_regions() {
    let (capabilities, _) = parse(&FakeMetadata::default().with_front_camera_three_a());
    assert_eq!(capabilities, front_camera().capabilities.three_a);
    assert_eq!(capabilities.af_modes, vec![AfModeSupport::Off]);
    assert_eq!(capabilities.max_regions.auto_focus, 0);
}

#[test]
fn full_characteristics_parse_carries_three_a_into_the_capabilities() {
    let parsed = parse_characteristics(
        &main_camera_characteristics(),
        camera_contract::raw_format::NativeMosaic::Unknown,
    )
    .unwrap();
    assert_eq!(parsed.capabilities.three_a, main_wide_camera().capabilities.three_a);
}

#[test]
fn camera_without_any_three_a_keys_is_honestly_unsupported_with_a_note_per_key() {
    let (capabilities, notes) = parse(&FakeMetadata::default());
    assert_eq!(capabilities, ThreeACapabilities::none());
    let noted: Vec<_> = notes.iter().map(|note| note.tag).collect();
    assert_eq!(
        noted,
        vec![
            "ACAMERA_CONTROL_AE_COMPENSATION_RANGE",
            "ACAMERA_CONTROL_AE_COMPENSATION_STEP",
            "ACAMERA_CONTROL_MAX_REGIONS",
            "ACAMERA_CONTROL_AF_AVAILABLE_MODES",
            "ACAMERA_CONTROL_AE_LOCK_AVAILABLE",
            "ACAMERA_CONTROL_AE_AVAILABLE_ANTIBANDING_MODES",
        ]
    );
    assert!(notes.iter().all(|note| note.issue == CharacteristicIssue::Absent));
}

#[test]
fn compensation_needs_both_range_and_step() {
    let (capabilities, notes) = parse(
        &FakeMetadata::default()
            .with_back_camera_three_a()
            .without(tags::AE_COMPENSATION_STEP),
    );
    assert_eq!(
        capabilities.exposure_compensation,
        ExposureCompensationSupport::NotSupported
    );
    assert_eq!(notes, vec![absent_note("ACAMERA_CONTROL_AE_COMPENSATION_STEP")]);
}

#[test]
fn zero_compensation_range_means_no_compensation_without_a_note() {
    let source = FakeMetadata::default()
        .with_back_camera_three_a()
        .int32s(tags::AE_COMPENSATION_RANGE, &[0, 0]);
    let (capabilities, notes) = parse(&source);
    assert_eq!(
        capabilities.exposure_compensation,
        ExposureCompensationSupport::NotSupported
    );
    assert_eq!(notes, Vec::new());
}

#[test]
fn inverted_range_or_non_positive_step_is_malformed() {
    for source in [
        FakeMetadata::default()
            .with_back_camera_three_a()
            .int32s(tags::AE_COMPENSATION_RANGE, &[18, -18]),
        FakeMetadata::default().with_back_camera_three_a().with(
            tags::AE_COMPENSATION_STEP,
            EntryValue::Rationals(vec![Rational {
                numerator: 1,
                denominator: 0,
            }]),
        ),
    ] {
        let (capabilities, notes) = parse(&source);
        assert_eq!(
            capabilities.exposure_compensation,
            ExposureCompensationSupport::NotSupported
        );
        assert_eq!(
            notes,
            vec![CharacteristicNote {
                tag: "ACAMERA_CONTROL_AE_COMPENSATION_RANGE",
                issue: CharacteristicIssue::Malformed {
                    problem: EntryProblem::OutOfRange
                }
            }]
        );
    }
}

#[test]
fn wrong_type_of_regions_is_malformed_and_gives_zero_regions() {
    let source = FakeMetadata::default()
        .with_back_camera_three_a()
        .bytes(tags::MAX_REGIONS, &[1, 0, 1]);
    let (capabilities, notes) = parse(&source);
    assert_eq!(capabilities.max_regions, MaxRegions::default());
    assert_eq!(
        notes,
        vec![CharacteristicNote {
            tag: "ACAMERA_CONTROL_MAX_REGIONS",
            issue: CharacteristicIssue::Malformed {
                problem: EntryProblem::WrongType {
                    expected: EntryKind::Int32,
                    actual: EntryKind::Byte
                }
            }
        }]
    );
}

#[test]
fn negative_or_short_regions_entry_is_not_trusted() {
    for values in [&[1, -1, 1][..], &[1, 0][..]] {
        let source = FakeMetadata::default()
            .with_back_camera_three_a()
            .int32s(tags::MAX_REGIONS, values);
        let (capabilities, notes) = parse(&source);
        assert_eq!(capabilities.max_regions, MaxRegions::default(), "{values:?}");
        assert_eq!(notes.len(), 1, "{values:?}");
    }
}

#[test]
fn unknown_af_and_antibanding_codes_are_skipped_with_a_note_not_silently() {
    let source = FakeMetadata::default()
        .with_back_camera_three_a()
        .bytes(tags::AF_AVAILABLE_MODES, &[0, 4, 77])
        .bytes(tags::AE_AVAILABLE_ANTIBANDING_MODES, &[1, 99]);
    let (capabilities, notes) = parse(&source);
    assert_eq!(
        capabilities.af_modes,
        vec![AfModeSupport::Off, AfModeSupport::ContinuousPicture]
    );
    assert_eq!(capabilities.antibanding_modes, vec![AntibandingModeSupport::Hz50]);
    let tags: Vec<_> = notes.iter().map(|note| note.tag).collect();
    assert_eq!(
        tags,
        vec![
            "ACAMERA_CONTROL_AF_AVAILABLE_MODES",
            "ACAMERA_CONTROL_AE_AVAILABLE_ANTIBANDING_MODES"
        ]
    );
}

#[test]
fn ae_lock_flag_maps_and_garbage_value_means_unavailable_with_a_note() {
    let flag = |value: u8| {
        parse(
            &FakeMetadata::default()
                .with_back_camera_three_a()
                .bytes(tags::AE_LOCK_AVAILABLE, &[value]),
        )
    };
    assert_eq!(flag(0).0.ae_lock, LockSupport::NotAvailable);
    assert_eq!(flag(0).1, Vec::new());
    assert_eq!(flag(1).0.ae_lock, LockSupport::Available);
    let (capabilities, notes) = flag(7);
    assert_eq!(capabilities.ae_lock, LockSupport::NotAvailable);
    assert_eq!(notes.len(), 1);
}

#[test]
fn unreadable_key_is_a_malformed_note_not_a_panic() {
    let source = FakeMetadata::default()
        .with_back_camera_three_a()
        .unreadable(tags::AF_AVAILABLE_MODES, -10000);
    let (capabilities, notes) = parse(&source);
    assert!(capabilities.af_modes.is_empty());
    assert_eq!(
        notes,
        vec![CharacteristicNote {
            tag: "ACAMERA_CONTROL_AF_AVAILABLE_MODES",
            issue: CharacteristicIssue::Malformed {
                problem: EntryProblem::Unreadable { status: -10000 }
            }
        }]
    );
}
