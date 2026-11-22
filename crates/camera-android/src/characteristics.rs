//! Разбор характеристик камеры (`ACameraMetadata` статических данных) в нейтральные
//! [`CameraCapabilities`].
//!
//! Правило: HAL не сообщил — это явный вариант контракта (`NotSupported`, `NotReported`,
//! пустой список) **и запись в [`CharacteristicNote`]**, а не ноль и не паника. Если без ключа
//! камеру описать нельзя (направление, уровень HAL, размеры массива), разбор возвращает
//! [`CharacteristicsError`], а бэкенд пропускает камеру с записью в диагностике.

use std::time::Duration;

use camera_contract::capabilities::{
    CameraCapabilities, ColorCalibrationSet, ExposureTimeRange, FocusCalibration, FocusCapability, FpsRange,
    FrameDuration, HardwareLevel, IsoRange, LensShading, ManualSensorRanges, Matrix3x3, Optics, PhysicalSizeMm,
    Rational, RawSensorInfo, RawStreamOption, RawSupport, ReferenceIlluminant, RequestCapability, SensorLevels,
    VendorProcessingFlags,
};
use camera_contract::flash::FlashUnit;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::{CameraId, CameraTopology, LensFacing};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, NativeMosaic, RawFormat};
use serde::Serialize;

use crate::entry::{
    EntryProblem, MetadataSource, Read, read_byte, read_bytes, read_float, read_floats, read_int32, read_int32s,
    read_int64s, read_rationals,
};
use crate::tags::{self, Tag};
use crate::three_a_characteristics::parse_three_a;

mod lens_calibration;
use lens_calibration::{parse_lens_calibration, parse_max_digital_zoom};

/// Без какого ключа камеру описать нельзя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CharacteristicsError {
    #[error("HAL не отдал обязательный ключ {}", tag.name)]
    MissingRequired { tag: Tag },
    #[error("ключ {} непригоден: {problem}", tag.name)]
    Malformed { tag: Tag, problem: EntryProblem },
    #[error("ключ {} содержит значение {value}, которого нет в контракте", tag.name)]
    UnknownValue { tag: Tag, value: i64 },
}

/// Что именно было не так с необязательным ключом.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "issue")]
pub enum CharacteristicIssue {
    /// HAL ключ не отдал.
    Absent,
    /// Ключ есть, но разобрать нельзя.
    Malformed { problem: EntryProblem },
    /// Подставлено значение по умолчанию (что именно — в `substitute`).
    Substituted { substitute: &'static str },
}

/// Заметка о деградации одного ключа — идёт в диагностику, чтобы «нет ключа» не исчезало молча.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CharacteristicNote {
    pub tag: &'static str,
    #[serde(flatten)]
    pub issue: CharacteristicIssue,
}

/// Итог разбора: возможности и заметки о том, чего HAL не дал.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCharacteristics {
    pub capabilities: CameraCapabilities,
    pub notes: Vec<CharacteristicNote>,
}

/// Накопитель заметок на время разбора (общий и для разбора возможностей 3A).
#[derive(Default)]
pub(crate) struct Notes(Vec<CharacteristicNote>);

impl Notes {
    /// Накопленные заметки отдельного разбора (для тестов разбора 3A).
    #[cfg(test)]
    pub(crate) fn into_notes(self) -> Vec<CharacteristicNote> {
        self.0
    }

    pub(crate) fn absent(&mut self, tag: Tag) {
        self.0.push(CharacteristicNote {
            tag: tag.name,
            issue: CharacteristicIssue::Absent,
        });
    }

    pub(crate) fn malformed(&mut self, tag: Tag, problem: EntryProblem) {
        self.0.push(CharacteristicNote {
            tag: tag.name,
            issue: CharacteristicIssue::Malformed { problem },
        });
    }

    pub(crate) fn substituted(&mut self, tag: Tag, substitute: &'static str) {
        self.0.push(CharacteristicNote {
            tag: tag.name,
            issue: CharacteristicIssue::Substituted { substitute },
        });
    }

    /// Необязательный ключ: отсутствие и непригодность превращаются в заметку и `None`.
    pub(crate) fn optional<T>(&mut self, tag: Tag, read: Read<T>) -> Option<T> {
        match read {
            Ok(Some(value)) => Some(value),
            Ok(None) => {
                self.absent(tag);
                None
            }
            Err(problem) => {
                self.malformed(tag, problem);
                None
            }
        }
    }
}

/// Обязательный ключ: отсутствие и непригодность — ошибка разбора.
fn required<T>(tag: Tag, read: Read<T>) -> Result<T, CharacteristicsError> {
    match read {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err(CharacteristicsError::MissingRequired { tag }),
        Err(problem) => Err(CharacteristicsError::Malformed { tag, problem }),
    }
}

/// Разбирает характеристики камеры.
///
/// `native_mosaic` — вендорское знание о родной мозаике сенсора: его даёт реестр вендорских
/// тегов, а не публичные характеристики (публичный RAW — уже биннинг до Bayer).
pub fn parse_characteristics(
    source: &dyn MetadataSource,
    native_mosaic: NativeMosaic,
) -> Result<ParsedCharacteristics, CharacteristicsError> {
    let mut notes = Notes::default();
    let facing = parse_facing(source)?;
    let hardware_level = parse_hardware_level(source)?;
    let request_capabilities = parse_request_capabilities(source, &mut notes);
    let topology = parse_topology(source, &request_capabilities, &mut notes);
    let pixel_array = parse_size(source, tags::PIXEL_ARRAY_SIZE)?;
    let active_array = parse_active_array(source)?;
    let sensor_orientation = parse_sensor_orientation(source)?;
    let optics = parse_optics(source, active_array, &mut notes)?;
    let has_raw = request_capabilities.contains(&RequestCapability::Raw);
    let has_manual_sensor = request_capabilities.contains(&RequestCapability::ManualSensor);

    let cfa = parse_cfa(source, has_raw)?;
    let raw_streams = if has_raw {
        parse_raw_streams(source, cfa.unwrap_or(CfaLayout::Rggb), &mut notes)?
    } else {
        Vec::new()
    };
    let raw = match (has_raw, cfa) {
        (true, Some(layout)) => {
            RawSupport::Supported(parse_raw_sensor_info(source, layout, native_mosaic, &mut notes)?)
        }
        _ => RawSupport::NotSupported,
    };

    let capabilities = CameraCapabilities {
        facing,
        topology,
        hardware_level,
        request_capabilities,
        pixel_array,
        active_array,
        sensor_orientation,
        raw_streams,
        ae_fps_ranges: parse_ae_fps_ranges(source, &mut notes),
        manual_sensor: parse_manual_sensor(source, has_manual_sensor, &mut notes),
        raw,
        three_a: parse_three_a(source, &mut notes),
        focus: parse_focus(source, &mut notes),
        flash: parse_flash_unit(source, &mut notes),
        optics,
        vendor_processing: VendorProcessingFlags {
            lens_shading: parse_lens_shading(source, &mut notes),
        },
    };
    Ok(ParsedCharacteristics {
        capabilities,
        notes: notes.0,
    })
}

fn parse_facing(source: &dyn MetadataSource) -> Result<LensFacing, CharacteristicsError> {
    let code = required(tags::LENS_FACING, read_byte(source, tags::LENS_FACING))?;
    match code {
        tags::LENS_FACING_FRONT => Ok(LensFacing::Front),
        tags::LENS_FACING_BACK => Ok(LensFacing::Back),
        tags::LENS_FACING_EXTERNAL => Ok(LensFacing::External),
        other => Err(CharacteristicsError::UnknownValue {
            tag: tags::LENS_FACING,
            value: i64::from(other),
        }),
    }
}

fn parse_hardware_level(source: &dyn MetadataSource) -> Result<HardwareLevel, CharacteristicsError> {
    let code = required(tags::HARDWARE_LEVEL, read_byte(source, tags::HARDWARE_LEVEL))?;
    match code {
        tags::HARDWARE_LEVEL_LEGACY => Ok(HardwareLevel::Legacy),
        tags::HARDWARE_LEVEL_LIMITED => Ok(HardwareLevel::Limited),
        tags::HARDWARE_LEVEL_FULL => Ok(HardwareLevel::Full),
        tags::HARDWARE_LEVEL_3 => Ok(HardwareLevel::Level3),
        tags::HARDWARE_LEVEL_EXTERNAL => Ok(HardwareLevel::External),
        other => Err(CharacteristicsError::UnknownValue {
            tag: tags::HARDWARE_LEVEL,
            value: i64::from(other),
        }),
    }
}

fn parse_request_capabilities(source: &dyn MetadataSource, notes: &mut Notes) -> Vec<RequestCapability> {
    let codes = notes.optional(
        tags::AVAILABLE_CAPABILITIES,
        read_bytes(source, tags::AVAILABLE_CAPABILITIES, 0),
    );
    codes
        .unwrap_or_default()
        .into_iter()
        .map(|code| match code {
            tags::CAPABILITY_BACKWARD_COMPATIBLE => RequestCapability::BackwardCompatible,
            tags::CAPABILITY_MANUAL_SENSOR => RequestCapability::ManualSensor,
            tags::CAPABILITY_MANUAL_POST_PROCESSING => RequestCapability::ManualPostProcessing,
            tags::CAPABILITY_RAW => RequestCapability::Raw,
            tags::CAPABILITY_READ_SENSOR_SETTINGS => RequestCapability::ReadSensorSettings,
            tags::CAPABILITY_BURST_CAPTURE => RequestCapability::BurstCapture,
            tags::CAPABILITY_LOGICAL_MULTI_CAMERA => RequestCapability::LogicalMultiCamera,
            tags::CAPABILITY_SYSTEM_CAMERA => RequestCapability::SystemCamera,
            other => RequestCapability::Other(other),
        })
        .collect()
}

/// Логическая камера: ID физических камер лежат в байтовой записи, строки через NUL.
fn parse_topology(
    source: &dyn MetadataSource,
    capabilities: &[RequestCapability],
    notes: &mut Notes,
) -> CameraTopology {
    if !capabilities.contains(&RequestCapability::LogicalMultiCamera) {
        return CameraTopology::Physical;
    }
    let physical_ids = notes
        .optional(
            tags::LOGICAL_PHYSICAL_IDS,
            read_bytes(source, tags::LOGICAL_PHYSICAL_IDS, 0),
        )
        .map(|bytes| split_physical_ids(&bytes))
        .unwrap_or_default();
    CameraTopology::Logical { physical_ids }
}

/// Делит `"2\03\0"` на ID; пустые куски и непригодные ID пропускает.
fn split_physical_ids(bytes: &[u8]) -> Vec<CameraId> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| String::from_utf8(chunk.to_vec()).ok())
        .filter_map(|id| CameraId::new(id).ok())
        .collect()
}

fn to_pixels(tag: Tag, value: i32) -> Result<u32, CharacteristicsError> {
    u32::try_from(value).map_err(|_| CharacteristicsError::Malformed {
        tag,
        problem: EntryProblem::OutOfRange,
    })
}

fn parse_size(source: &dyn MetadataSource, tag: Tag) -> Result<PixelSize, CharacteristicsError> {
    let values = required(tag, read_int32s(source, tag, 2))?;
    Ok(PixelSize::new(to_pixels(tag, values[0])?, to_pixels(tag, values[1])?))
}

/// `ACAMERA_SENSOR_INFO_ACTIVE_ARRAY_SIZE`: `(left, top, width, height)`, не `right/bottom`.
fn parse_active_array(source: &dyn MetadataSource) -> Result<PixelRect, CharacteristicsError> {
    parse_array_rect(source, tags::ACTIVE_ARRAY_SIZE)
}

/// Прямоугольник сенсора формата `int32[4]` `(left, top, width, height)`: общий для активной
/// области и `PRE_CORRECTION_ACTIVE_ARRAY_SIZE`. Ключ обязателен — снисхождение к отсутствию
/// решает вызывающий.
fn parse_array_rect(source: &dyn MetadataSource, tag: Tag) -> Result<PixelRect, CharacteristicsError> {
    let values = required(tag, read_int32s(source, tag, 4))?;
    Ok(PixelRect {
        left: to_pixels(tag, values[0])?,
        top: to_pixels(tag, values[1])?,
        size: PixelSize::new(to_pixels(tag, values[2])?, to_pixels(tag, values[3])?),
    })
}

/// `ACAMERA_SENSOR_ORIENTATION`: обязателен (без него кадр нельзя повернуть к экрану). Camera2
/// допускает только 0/90/180/270; другое число — `UnknownValue`, а не округление.
fn parse_sensor_orientation(source: &dyn MetadataSource) -> Result<SensorOrientation, CharacteristicsError> {
    let tag = tags::SENSOR_ORIENTATION;
    let degrees = required(tag, read_int32(source, tag))?;
    SensorOrientation::from_degrees(degrees).ok_or(CharacteristicsError::UnknownValue {
        tag,
        value: i64::from(degrees),
    })
}

fn parse_optics(
    source: &dyn MetadataSource,
    active_array: PixelRect,
    notes: &mut Notes,
) -> Result<Optics, CharacteristicsError> {
    let size = required(tags::PHYSICAL_SIZE, read_floats(source, tags::PHYSICAL_SIZE, 2))?;
    Ok(Optics {
        apertures_f_number: notes
            .optional(
                tags::AVAILABLE_APERTURES,
                read_floats(source, tags::AVAILABLE_APERTURES, 0),
            )
            .unwrap_or_default(),
        focal_lengths_mm: notes
            .optional(
                tags::AVAILABLE_FOCAL_LENGTHS,
                read_floats(source, tags::AVAILABLE_FOCAL_LENGTHS, 0),
            )
            .unwrap_or_default(),
        sensor_physical_size: PhysicalSizeMm {
            width_mm: size[0],
            height_mm: size[1],
        },
        calibration: parse_lens_calibration(source, active_array, notes),
        max_digital_zoom: parse_max_digital_zoom(source, notes),
    })
}

/// Раскладка CFA: обязательна для камеры с RAW; у камеры без RAW отсутствие не ошибка.
fn parse_cfa(source: &dyn MetadataSource, has_raw: bool) -> Result<Option<CfaLayout>, CharacteristicsError> {
    let tag = tags::COLOR_FILTER_ARRANGEMENT;
    let code = match read_byte(source, tag) {
        Ok(Some(code)) => code,
        Ok(None) if !has_raw => return Ok(None),
        Ok(None) => return Err(CharacteristicsError::MissingRequired { tag }),
        Err(problem) => return Err(CharacteristicsError::Malformed { tag, problem }),
    };
    let layout = match code {
        tags::CFA_RGGB => CfaLayout::Rggb,
        tags::CFA_GRBG => CfaLayout::Grbg,
        tags::CFA_GBRG => CfaLayout::Gbrg,
        tags::CFA_BGGR => CfaLayout::Bggr,
        tags::CFA_RGB => CfaLayout::Rgb,
        tags::CFA_MONO => CfaLayout::Mono,
        tags::CFA_NIR => CfaLayout::Nir,
        other => {
            return Err(CharacteristicsError::UnknownValue {
                tag,
                value: i64::from(other),
            });
        }
    };
    Ok(Some(layout))
}

/// Публичные выходные RAW10/RAW16 из карты потоков (`[format, w, h, isInput] × n`),
/// минимальная длительность кадра — из `[format, w, h, duration] × n`.
fn parse_raw_streams(
    source: &dyn MetadataSource,
    layout: CfaLayout,
    notes: &mut Notes,
) -> Result<Vec<RawStreamOption>, CharacteristicsError> {
    let tag = tags::STREAM_CONFIGURATIONS;
    let configurations = required(tag, read_int32s(source, tag, 0))?;
    let durations = notes
        .optional(
            tags::MIN_FRAME_DURATIONS,
            read_int64s(source, tags::MIN_FRAME_DURATIONS, 0),
        )
        .unwrap_or_default();
    let mosaic = MosaicPattern {
        kind: MosaicKind::Bayer,
        layout,
    };
    let mut options = Vec::new();
    let mut without_duration = false;
    for entry in configurations.chunks_exact(4) {
        let (format_code, width, height, direction) = (entry[0], entry[1], entry[2], entry[3]);
        if direction != tags::STREAM_CONFIGURATION_OUTPUT {
            continue;
        }
        let format = match format_code {
            tags::IMAGE_FORMAT_RAW10 => RawFormat::Raw10,
            tags::IMAGE_FORMAT_RAW16 => RawFormat::Raw16,
            _ => continue,
        };
        let size = PixelSize::new(to_pixels(tag, width)?, to_pixels(tag, height)?);
        let duration = durations
            .chunks_exact(4)
            .find(|candidate| {
                candidate[0] == i64::from(format_code)
                    && candidate[1] == i64::from(width)
                    && candidate[2] == i64::from(height)
            })
            .and_then(|candidate| u64::try_from(candidate[3]).ok());
        let min_frame_duration = match duration {
            Some(nanos) => FrameDuration::from_nanos(nanos),
            None => {
                without_duration = true;
                FrameDuration::from_nanos(0)
            }
        };
        options.push(RawStreamOption {
            format,
            size,
            mosaic,
            min_frame_duration,
        });
    }
    if without_duration {
        notes.substituted(
            tags::MIN_FRAME_DURATIONS,
            "нулевая длительность («HAL не ограничил») для RAW-размеров без записи",
        );
    }
    Ok(options)
}

/// Уровни, CFA и калибровка цвета RAW-камеры.
fn parse_raw_sensor_info(
    source: &dyn MetadataSource,
    layout: CfaLayout,
    native_mosaic: NativeMosaic,
    notes: &mut Notes,
) -> Result<RawSensorInfo, CharacteristicsError> {
    let black = required(
        tags::BLACK_LEVEL_PATTERN,
        read_int32s(source, tags::BLACK_LEVEL_PATTERN, 4),
    )?;
    let mut black_level_pattern = [0_u32; 4];
    for (target, value) in black_level_pattern.iter_mut().zip(&black) {
        *target = to_pixels(tags::BLACK_LEVEL_PATTERN, *value)?;
    }
    let white = required(tags::WHITE_LEVEL, read_int32(source, tags::WHITE_LEVEL))?;
    Ok(RawSensorInfo {
        levels: SensorLevels {
            black_level_pattern,
            white_level: to_pixels(tags::WHITE_LEVEL, white)?,
        },
        cfa: layout,
        native_mosaic,
        color_calibration: parse_color_calibration(source, notes),
    })
}

/// Два набора (эталонные источники 1 и 2). Набор без иллюминанта, цветовой или прямой матрицы
/// пропускается с заметкой; отсутствующая поправка экземпляра = единичная (так задаёт DNG).
fn parse_color_calibration(source: &dyn MetadataSource, notes: &mut Notes) -> Vec<ColorCalibrationSet> {
    let sets = [
        (
            tags::REFERENCE_ILLUMINANT1,
            tags::COLOR_TRANSFORM1,
            tags::FORWARD_MATRIX1,
            tags::CALIBRATION_TRANSFORM1,
        ),
        (
            tags::REFERENCE_ILLUMINANT2,
            tags::COLOR_TRANSFORM2,
            tags::FORWARD_MATRIX2,
            tags::CALIBRATION_TRANSFORM2,
        ),
    ];
    let mut result = Vec::new();
    for (illuminant_tag, color_tag, forward_tag, calibration_tag) in sets {
        let illuminant = notes.optional(illuminant_tag, read_byte(source, illuminant_tag));
        let color = notes.optional(color_tag, read_matrix(source, color_tag));
        let forward = notes.optional(forward_tag, read_matrix(source, forward_tag));
        let (Some(illuminant), Some(color_matrix), Some(forward_matrix)) = (illuminant, color, forward) else {
            continue;
        };
        let calibration_transform = match read_matrix(source, calibration_tag) {
            Ok(Some(matrix)) => matrix,
            Ok(None) => {
                notes.substituted(calibration_tag, "единичная матрица (DNG: поправка по умолчанию)");
                identity_matrix()
            }
            Err(problem) => {
                notes.malformed(calibration_tag, problem);
                identity_matrix()
            }
        };
        result.push(ColorCalibrationSet {
            illuminant: ReferenceIlluminant::from_exif_code(illuminant),
            color_matrix,
            forward_matrix,
            calibration_transform,
        });
    }
    result
}

fn read_matrix(source: &dyn MetadataSource, tag: Tag) -> Read<Matrix3x3> {
    Ok(read_rationals(source, tag, 9)?.map(|values| {
        let mut matrix = [Rational {
            numerator: 0,
            denominator: 1,
        }; 9];
        matrix.copy_from_slice(&values[..9]);
        Matrix3x3(matrix)
    }))
}

fn identity_matrix() -> Matrix3x3 {
    let element = |numerator| Rational {
        numerator,
        denominator: 1,
    };
    Matrix3x3([1, 0, 0, 0, 1, 0, 0, 0, 1].map(element))
}

fn parse_ae_fps_ranges(source: &dyn MetadataSource, notes: &mut Notes) -> Vec<FpsRange> {
    let values = notes
        .optional(
            tags::AE_AVAILABLE_FPS_RANGES,
            read_int32s(source, tags::AE_AVAILABLE_FPS_RANGES, 0),
        )
        .unwrap_or_default();
    values
        .chunks_exact(2)
        .filter_map(|pair| {
            Some(FpsRange {
                min: u32::try_from(pair[0]).ok()?,
                max: u32::try_from(pair[1]).ok()?,
            })
        })
        .collect()
}

/// Ручной сенсор: нужен capability `MANUAL_SENSOR` и все три диапазона; иначе `NotSupported`
/// (с заметкой, если capability объявлен, а диапазонов нет).
fn parse_manual_sensor(source: &dyn MetadataSource, declared: bool, notes: &mut Notes) -> ManualSensorRanges {
    if !declared {
        return ManualSensorRanges::NotSupported;
    }
    let iso = notes.optional(tags::SENSITIVITY_RANGE, read_int32s(source, tags::SENSITIVITY_RANGE, 2));
    let exposure = notes.optional(
        tags::EXPOSURE_TIME_RANGE,
        read_int64s(source, tags::EXPOSURE_TIME_RANGE, 2),
    );
    let max_frame = notes.optional(
        tags::MAX_FRAME_DURATION,
        read_int64s(source, tags::MAX_FRAME_DURATION, 1),
    );
    let (Some(iso), Some(exposure), Some(max_frame)) = (iso, exposure, max_frame) else {
        return ManualSensorRanges::NotSupported;
    };
    let ranges = (
        u32::try_from(iso[0]),
        u32::try_from(iso[1]),
        u64::try_from(exposure[0]),
        u64::try_from(exposure[1]),
        u64::try_from(max_frame[0]),
    );
    let (Ok(iso_min), Ok(iso_max), Ok(shortest), Ok(longest), Ok(max_frame_nanos)) = ranges else {
        notes.malformed(tags::SENSITIVITY_RANGE, EntryProblem::OutOfRange);
        return ManualSensorRanges::NotSupported;
    };
    ManualSensorRanges::Supported {
        iso: IsoRange {
            min: iso_min,
            max: iso_max,
        },
        exposure_time: ExposureTimeRange {
            shortest: Duration::from_nanos(shortest),
            longest: Duration::from_nanos(longest),
        },
        max_frame_duration: FrameDuration::from_nanos(max_frame_nanos),
    }
}

/// Фокус: нулевая или отсутствующая минимальная дистанция — фиксированный фокус (фронт OnePlus 11).
fn parse_focus(source: &dyn MetadataSource, notes: &mut Notes) -> FocusCapability {
    let minimum = notes.optional(tags::MIN_FOCUS_DISTANCE, read_float(source, tags::MIN_FOCUS_DISTANCE));
    let Some(min_focus_distance_diopters) = minimum.filter(|value| *value > 0.0) else {
        return FocusCapability::Fixed;
    };
    // Гиперфокал необязателен: нет ключа — честное `None` (заметку об отсутствии пишет `optional`).
    let hyperfocal_distance_diopters =
        notes.optional(tags::HYPERFOCAL_DISTANCE, read_float(source, tags::HYPERFOCAL_DISTANCE));
    let calibration = match notes.optional(
        tags::FOCUS_DISTANCE_CALIBRATION,
        read_byte(source, tags::FOCUS_DISTANCE_CALIBRATION),
    ) {
        Some(tags::FOCUS_CALIBRATION_CALIBRATED) => FocusCalibration::Calibrated,
        Some(tags::FOCUS_CALIBRATION_APPROXIMATE) => FocusCalibration::Approximate,
        Some(_) | None => FocusCalibration::Uncalibrated,
    };
    FocusCapability::Adjustable {
        min_focus_distance_diopters,
        hyperfocal_distance_diopters,
        calibration,
    }
}

/// Вспышка: ключ отсутствует или непригоден — вспышки нет (честная заметка, не молчание);
/// незнакомое значение булева тоже считается «нет».
fn parse_flash_unit(source: &dyn MetadataSource, notes: &mut Notes) -> FlashUnit {
    match notes.optional(
        tags::FLASH_INFO_AVAILABLE,
        read_byte(source, tags::FLASH_INFO_AVAILABLE),
    ) {
        Some(tags::FLASH_INFO_AVAILABLE_TRUE) => FlashUnit::Present,
        Some(_) | None => FlashUnit::Absent,
    }
}

fn parse_lens_shading(source: &dyn MetadataSource, notes: &mut Notes) -> LensShading {
    match notes.optional(
        tags::LENS_SHADING_APPLIED,
        read_byte(source, tags::LENS_SHADING_APPLIED),
    ) {
        Some(tags::LENS_SHADING_APPLIED_TRUE) => LensShading::AppliedByVendor,
        Some(tags::LENS_SHADING_APPLIED_FALSE) => LensShading::NotApplied,
        Some(_) | None => LensShading::NotReported,
    }
}

#[cfg(test)]
#[path = "characteristics_tests.rs"]
mod tests;
