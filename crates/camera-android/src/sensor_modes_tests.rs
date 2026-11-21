use camera_contract::fixtures::camera_id;

use super::*;
use crate::config::AndroidCameraConfig;
use crate::fake_metadata::{FRONT_TABLE, FakeMetadata, MAIN_TABLE, ULTRA_WIDE_TABLE};
use crate::vendor::{TagResolution, TagResolver};

/// Номер тега таблицы на прошивке OnePlus 11 (в коде бэкенда его нет — только имя).
const TABLE_TAG_CODE: u32 = 0x803a_0000;

fn fps(value: u32) -> TargetFrameRate {
    TargetFrameRate::new(value).unwrap()
}

#[test]
fn real_front_table_parses_to_its_four_modes() {
    let table = SensorModeTable::parse(&FRONT_TABLE).unwrap();
    let sizes: Vec<_> = table
        .modes()
        .iter()
        .map(|mode| (mode.size.width, mode.size.height, mode.max_frames_per_second))
        .collect();
    assert_eq!(
        sizes,
        [(4608, 3456, 30), (2304, 1728, 30), (2304, 1296, 30), (2304, 1728, 30)]
    );
}

#[test]
fn real_main_and_ultra_wide_tables_parse_with_every_mode() {
    assert_eq!(SensorModeTable::parse(&MAIN_TABLE).unwrap().modes().len(), 13);
    let ultra = SensorModeTable::parse(&ULTRA_WIDE_TABLE).unwrap();
    assert_eq!(ultra.modes().len(), 6);
    assert_eq!(
        ultra.modes()[2],
        SensorMode {
            size: PixelSize::new(4000, 3000),
            max_frames_per_second: 60
        }
    );
}

#[test]
fn native_mode_needs_an_exact_size_and_enough_frame_rate() {
    let ultra = SensorModeTable::parse(&ULTRA_WIDE_TABLE).unwrap();
    let full = PixelSize::new(4000, 3000);
    assert_eq!(ultra.native_mode_support(full, fps(60)), NativeModeSupport::Offered);
    assert_eq!(ultra.native_mode_support(full, fps(30)), NativeModeSupport::Offered);
    // Частота выше максимума режима.
    assert_eq!(ultra.native_mode_support(full, fps(120)), NativeModeSupport::NotOffered);
    // Размер, которого в таблице нет (даже если рядом есть режим побольше).
    assert_eq!(
        ultra.native_mode_support(PixelSize::new(2000, 1500), fps(30)),
        NativeModeSupport::NotOffered
    );
    // Размер есть, но только на 24 к/с.
    assert_eq!(
        ultra.native_mode_support(PixelSize::new(2832, 2124), fps(30)),
        NativeModeSupport::NotOffered
    );
}

#[test]
fn malformed_arrays_fail_with_distinguishable_errors() {
    assert_eq!(
        SensorModeTable::parse(&[1, 4, 10, 10, 30, 1]),
        Err(SensorModeParseError::UnsupportedFieldCount { actual: 4 })
    );
    assert_eq!(
        SensorModeTable::parse(&[0, 3]),
        Err(SensorModeParseError::NonPositiveModeCount { declared: 0 })
    );
    assert_eq!(
        SensorModeTable::parse(&[-1, 3]),
        Err(SensorModeParseError::NonPositiveModeCount { declared: -1 })
    );
    assert_eq!(
        SensorModeTable::parse(&[2, 3, 10, 10, 30]),
        Err(SensorModeParseError::LengthMismatch { expected: 8, actual: 5 })
    );
    assert_eq!(
        SensorModeTable::parse(&[1, 3, 10, 10, 30, 99]),
        Err(SensorModeParseError::LengthMismatch { expected: 5, actual: 6 })
    );
    assert_eq!(
        SensorModeTable::parse(&[2, 3, 10, 10, 30, 10, 0, 30]),
        Err(SensorModeParseError::NonPositiveValue { mode_index: 1 })
    );
    assert_eq!(
        SensorModeTable::parse(&[1, 3, 10, 10, -5]),
        Err(SensorModeParseError::NonPositiveValue { mode_index: 0 })
    );
    // Огромное число режимов не переполняет проверку длины.
    assert!(matches!(
        SensorModeTable::parse(&[i32::MAX, 3, 1, 1, 1]),
        Err(SensorModeParseError::LengthMismatch { .. })
    ));
}

/// Устройство: знает тег таблицы, не знает его или падает на разрешении имени.
enum Firmware {
    KnowsTable,
    DoesNotKnowTable,
    ResolveFails(i32),
}

impl TagResolver for Firmware {
    fn resolve_tag(&self, name: &str) -> TagResolution {
        if name != SENSOR_MODE_TABLE_TAG_NAME {
            return TagResolution::NotFound;
        }
        match self {
            Firmware::KnowsTable => TagResolution::Found { tag: TABLE_TAG_CODE },
            Firmware::DoesNotKnowTable => TagResolution::NotFound,
            Firmware::ResolveFails(status) => TagResolution::Failed { status: *status },
        }
    }
}

fn table_tag() -> Tag {
    Tag {
        code: TABLE_TAG_CODE,
        name: "test",
    }
}

fn read_from(firmware: &Firmware, metadata: &FakeMetadata) -> SensorModeAvailability {
    let vendor = VendorCapabilities::resolve(&camera_id("1"), &AndroidCameraConfig::default(), firmware);
    SensorModeAvailability::read(metadata, &vendor)
}

#[test]
fn a_present_table_is_read_by_the_name_resolved_tag() {
    let metadata = FakeMetadata::default().int32s(table_tag(), &FRONT_TABLE);
    let SensorModeAvailability::Listed(table) = read_from(&Firmware::KnowsTable, &metadata) else {
        panic!("таблица должна быть прочитана");
    };
    assert_eq!(table, SensorModeTable::parse(&FRONT_TABLE).unwrap());
}

#[test]
fn unknown_tag_or_missing_entry_is_absent_on_device() {
    let with_entry = FakeMetadata::default().int32s(table_tag(), &FRONT_TABLE);
    assert_eq!(
        read_from(&Firmware::DoesNotKnowTable, &with_entry),
        SensorModeAvailability::AbsentOnDevice
    );
    assert_eq!(
        read_from(&Firmware::KnowsTable, &FakeMetadata::default()),
        SensorModeAvailability::AbsentOnDevice
    );
}

#[test]
fn wrong_type_unreadable_resolve_failure_and_malformed_data_are_unusable_with_a_reason() {
    let wrong_type = FakeMetadata::default().bytes(table_tag(), &[1, 2, 3]);
    assert!(matches!(
        read_from(&Firmware::KnowsTable, &wrong_type),
        SensorModeAvailability::Unusable(SensorModeProblem::Entry(EntryProblem::WrongType { .. }))
    ));
    let unreadable = FakeMetadata::default().unreadable(table_tag(), -7);
    assert_eq!(
        read_from(&Firmware::KnowsTable, &unreadable),
        SensorModeAvailability::Unusable(SensorModeProblem::Entry(EntryProblem::Unreadable { status: -7 }))
    );
    assert_eq!(
        read_from(&Firmware::ResolveFails(-3), &FakeMetadata::default()),
        SensorModeAvailability::Unusable(SensorModeProblem::TagResolveFailed { status: -3 })
    );
    let malformed = FakeMetadata::default().int32s(table_tag(), &[2, 3, 10, 10, 30]);
    assert_eq!(
        read_from(&Firmware::KnowsTable, &malformed),
        SensorModeAvailability::Unusable(SensorModeProblem::Malformed(SensorModeParseError::LengthMismatch {
            expected: 8,
            actual: 5
        }))
    );
}

#[test]
fn only_a_listed_table_can_offer_a_mode() {
    let size = PixelSize::new(4000, 3000);
    let listed = SensorModeAvailability::Listed(SensorModeTable::parse(&ULTRA_WIDE_TABLE).unwrap());
    assert_eq!(listed.native_mode_support(size, fps(60)), NativeModeSupport::Offered);
    let unusable = SensorModeAvailability::Unusable(SensorModeProblem::TagResolveFailed { status: 1 });
    for availability in [SensorModeAvailability::AbsentOnDevice, unusable] {
        assert_eq!(
            availability.native_mode_support(size, fps(60)),
            NativeModeSupport::NotOffered
        );
    }
}
