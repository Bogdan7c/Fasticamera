//! Фейковые метаданные для тестов на ПК: карта «тег → запись» с числами OnePlus 11 из сессии 00.

use std::collections::HashMap;

use camera_contract::capabilities::Rational;

use crate::entry::{EntryLookup, EntryValue, MetadataSource};
use crate::tags::{self, Tag};

/// Реальные массивы `SensorModeTable` с OnePlus 11 (`dumpsys media.camera`).
pub const FRONT_TABLE: [i32; 14] = [4, 3, 4608, 3456, 30, 2304, 1728, 30, 2304, 1296, 30, 2304, 1728, 30];
pub const MAIN_TABLE: [i32; 41] = [
    13, 3, 8192, 6144, 24, 4096, 3072, 30, 4096, 3072, 30, 4096, 2304, 30, 4096, 2304, 60, 1600, 1200, 24, 4096, 3072,
    24, 2048, 1152, 240, 1296, 736, 480, 4096, 2304, 120, 7680, 4320, 24, 4096, 2304, 30, 4096, 2304, 30,
];
pub const ULTRA_WIDE_TABLE: [i32; 20] = [
    6, 3, 8000, 6000, 24, 4000, 3000, 30, 4000, 3000, 60, 4000, 2250, 30, 4000, 2250, 60, 2832, 2124, 24,
];

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
        // Задняя камера: сенсор повёрнут на 90° к естественной ориентации телефона.
        .int32s(tags::SENSOR_ORIENTATION, &[90])
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
        .floats(tags::AVAILABLE_FOCAL_LENGTHS, &[5.59])
        .floats(tags::PHYSICAL_SIZE, &[8.192, 6.144])
        .bytes(tags::LENS_SHADING_APPLIED, &[tags::LENS_SHADING_APPLIED_TRUE])
        // Калибровка объектива: `dumpsys media.camera` OnePlus 11, 20.11.2026 (сессия 13).
        .floats(
            tags::LENS_INTRINSIC_CALIBRATION,
            &[2865.7437, 2865.9556, 2040.2867, 1515.4318, 0.0],
        )
        .floats(tags::LENS_POSE_ROTATION, &[1.0, 0.0, 0.0, 0.0])
        .floats(tags::LENS_POSE_TRANSLATION, &[0.0, 0.0, 0.0])
        .bytes(tags::LENS_POSE_REFERENCE, &[tags::LENS_POSE_REFERENCE_PRIMARY_CAMERA])
        .int32s(tags::PRE_CORRECTION_ACTIVE_ARRAY_SIZE, &[0, 0, width, height])
        .floats(tags::MAX_DIGITAL_ZOOM, &[10.0])
        // Вспышка есть у задних камер OnePlus 11 (зонд `flash` 14d).
        .bytes(tags::FLASH_INFO_AVAILABLE, &[tags::FLASH_INFO_AVAILABLE_TRUE])
        .with_back_camera_three_a()
}

/// Результат кадра с состоянием 3A: ровно те ключи, которые нужны `three_a_result`.
pub fn three_a_result_fixture() -> FakeMetadata {
    FakeMetadata::default()
        .bytes(tags::AF_STATE, &[tags::AF_STATE_FOCUSED_LOCKED])
        .bytes(tags::AE_STATE, &[tags::AE_STATE_CONVERGED])
        .bytes(tags::LENS_STATE, &[tags::LENS_STATE_STATIONARY])
        .floats(tags::LENS_FOCUS_DISTANCE, &[2.5])
        .bytes(tags::SCENE_FLICKER, &[tags::SCENE_FLICKER_50HZ])
        .bytes(tags::FLASH_MODE, &[tags::FLASH_MODE_OFF])
        .bytes(tags::FLASH_STATE, &[tags::FLASH_STATE_READY])
}

impl FakeMetadata {
    /// 3A задней камеры OnePlus 11 по `dumpsys media.camera`: компенсация −18…18 по 1/6, регионы
    /// [AE 1, AWB 0, AF 1], режимы AF 0–4, блокировка AE есть, антибандинг 0–3.
    pub fn with_back_camera_three_a(self) -> Self {
        self.int32s(tags::AE_COMPENSATION_RANGE, &[-18, 18])
            .with(
                tags::AE_COMPENSATION_STEP,
                EntryValue::Rationals(vec![Rational {
                    numerator: 1,
                    denominator: 6,
                }]),
            )
            .int32s(tags::MAX_REGIONS, &[1, 0, 1])
            .bytes(tags::AF_AVAILABLE_MODES, &[0, 1, 2, 3, 4])
            .bytes(tags::AE_LOCK_AVAILABLE, &[tags::AE_LOCK_AVAILABLE_TRUE])
            .bytes(tags::AE_AVAILABLE_ANTIBANDING_MODES, &[0, 1, 2, 3])
    }

    /// 3A фронтальной камеры: фиксированный фокус — только `AF_MODE_OFF`, регионов AF нет.
    pub fn with_front_camera_three_a(self) -> Self {
        self.with_back_camera_three_a()
            .int32s(tags::MAX_REGIONS, &[1, 0, 0])
            .bytes(tags::AF_AVAILABLE_MODES, &[0])
    }
}
