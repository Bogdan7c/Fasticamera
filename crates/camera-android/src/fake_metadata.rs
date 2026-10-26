//! Фейковые метаданные для тестов на ПК: карта «тег → запись» с числами OnePlus 11 из сессии 00.

use std::collections::HashMap;

use camera_contract::capabilities::Rational;

use crate::entry::{EntryLookup, EntryValue, MetadataSource};
use crate::tags::{self, Tag};

/// Карта тегов; отсутствующий тег = «HAL не отдал».
#[derive(Debug, Clone, Default)]
pub struct FakeMetadata {
    entries: HashMap<u32, EntryLookup>,
}

impl FakeMetadata {
    pub fn with(mut self, tag: Tag, value: EntryValue) -> Self {
        self.entries.insert(tag.code, EntryLookup::Present(value));
        self
    }

    pub fn without(mut self, tag: Tag) -> Self {
        self.entries.remove(&tag.code);
        self
    }

    pub fn unreadable(mut self, tag: Tag, status: i32) -> Self {
        self.entries.insert(tag.code, EntryLookup::Unreadable { status });
        self
    }

    pub fn bytes(self, tag: Tag, values: &[u8]) -> Self {
        self.with(tag, EntryValue::Bytes(values.to_vec()))
    }

    pub fn int32s(self, tag: Tag, values: &[i32]) -> Self {
        self.with(tag, EntryValue::Int32s(values.to_vec()))
    }

    pub fn int64s(self, tag: Tag, values: &[i64]) -> Self {
        self.with(tag, EntryValue::Int64s(values.to_vec()))
    }

    pub fn floats(self, tag: Tag, values: &[f32]) -> Self {
        self.with(tag, EntryValue::Floats(values.to_vec()))
    }

    pub fn identity_matrix(self, tag: Tag) -> Self {
        let rational = |numerator| Rational {
            numerator,
            denominator: 1,
        };
        let values = [1, 0, 0, 0, 1, 0, 0, 0, 1].map(rational);
        self.with(tag, EntryValue::Rationals(values.to_vec()))
    }
}

impl MetadataSource for FakeMetadata {
    fn entry(&self, tag: Tag) -> EntryLookup {
        self.entries.get(&tag.code).cloned().unwrap_or(EntryLookup::Absent)
    }
}

/// Размер RAW-буфера основной камеры OnePlus 11 (00 C1).
pub const MAIN_RAW_WIDTH: i32 = 4096;
pub const MAIN_RAW_HEIGHT: i32 = 3072;
/// 33,3 мс: публично объявленный потолок (00 C1).
pub const PUBLIC_MIN_FRAME_DURATION_NS: i64 = 33_333_333;

/// Характеристики основной камеры «2» по фактам 00 (сходятся с `camera_contract::fixtures::main_wide_camera`).
pub fn main_camera_characteristics() -> FakeMetadata {
    let [raw16_format, raw10_format] = [0x20, 0x25];
    let (width, height) = (MAIN_RAW_WIDTH, MAIN_RAW_HEIGHT);
    FakeMetadata::default()
        .bytes(tags::LENS_FACING, &[tags::LENS_FACING_BACK])
        .bytes(tags::HARDWARE_LEVEL, &[tags::HARDWARE_LEVEL_3])
        .bytes(
            tags::AVAILABLE_CAPABILITIES,
            &[
                tags::CAPABILITY_BACKWARD_COMPATIBLE,
                tags::CAPABILITY_MANUAL_SENSOR,
                tags::CAPABILITY_RAW,
                tags::CAPABILITY_READ_SENSOR_SETTINGS,
            ],
        )
        .int32s(tags::PIXEL_ARRAY_SIZE, &[width, height])
        .int32s(tags::ACTIVE_ARRAY_SIZE, &[0, 0, width, height])
        .int32s(
            tags::STREAM_CONFIGURATIONS,
            &[
                // Выходной RAW10 и RAW16, плюс постороннее: JPEG (0x100) и вход (isInput=1) — разбор обязан их пропустить.
                raw10_format,
                width,
                height,
                tags::STREAM_CONFIGURATION_OUTPUT,
                raw16_format,
                width,
                height,
                tags::STREAM_CONFIGURATION_OUTPUT,
                0x100,
                width,
                height,
                tags::STREAM_CONFIGURATION_OUTPUT,
                raw16_format,
                2048,
                1536,
                tags::STREAM_CONFIGURATION_INPUT,
            ],
        )
        .int64s(
            tags::MIN_FRAME_DURATIONS,
            &[
                i64::from(raw16_format),
                i64::from(width),
                i64::from(height),
                PUBLIC_MIN_FRAME_DURATION_NS,
                i64::from(raw10_format),
                i64::from(width),
                i64::from(height),
                PUBLIC_MIN_FRAME_DURATION_NS,
            ],
        )
        .int32s(tags::AE_AVAILABLE_FPS_RANGES, &[15, 30, 30, 30])
        .int32s(tags::SENSITIVITY_RANGE, &[100, 6400])
        .int64s(tags::EXPOSURE_TIME_RANGE, &[10_000, 32_000_000_000])
        .int64s(tags::MAX_FRAME_DURATION, &[32_000_000_000])
        .int32s(tags::BLACK_LEVEL_PATTERN, &[64, 64, 64, 64])
        .int32s(tags::WHITE_LEVEL, &[1023])
        .bytes(tags::COLOR_FILTER_ARRANGEMENT, &[tags::CFA_BGGR])
        .bytes(tags::REFERENCE_ILLUMINANT1, &[21])
        .bytes(tags::REFERENCE_ILLUMINANT2, &[17])
        .identity_matrix(tags::COLOR_TRANSFORM1)
        .identity_matrix(tags::COLOR_TRANSFORM2)
        .identity_matrix(tags::FORWARD_MATRIX1)
        .identity_matrix(tags::FORWARD_MATRIX2)
        .identity_matrix(tags::CALIBRATION_TRANSFORM1)
        .identity_matrix(tags::CALIBRATION_TRANSFORM2)
        .floats(tags::MIN_FOCUS_DISTANCE, &[10.0])
        .floats(tags::HYPERFOCAL_DISTANCE, &[0.3])
        .bytes(tags::FOCUS_DISTANCE_CALIBRATION, &[tags::FOCUS_CALIBRATION_APPROXIMATE])
        .floats(tags::AVAILABLE_APERTURES, &[1.8])
        .floats(tags::AVAILABLE_FOCAL_LENGTHS, &[5.6])
        .floats(tags::PHYSICAL_SIZE, &[9.8, 7.4])
        .bytes(tags::LENS_SHADING_APPLIED, &[tags::LENS_SHADING_APPLIED_TRUE])
}
