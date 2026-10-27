//! Возможности камеры в нейтральных типах: что умеет сенсор, оптика и HAL.
//!
//! Заполняет `camera-android` из `ACameraMetadata` характеристик. Всё, чего HAL не сообщил,
//! представлено явным вариантом (`NotSupported`, `NotReported`), а не нулями и не догадкой.

use std::time::Duration;

use serde::Serialize;

use crate::geometry::{PixelRect, PixelSize, SensorOrientation};
use crate::ids::{CameraId, CameraTopology, LensFacing};
use crate::raw_format::{CfaLayout, MosaicPattern, NativeMosaic, RawFormat};

/// Длительность кадра (время от начала одного кадра до начала следующего).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FrameDuration(u64);

impl FrameDuration {
    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub const fn as_duration(self) -> Duration {
        Duration::from_nanos(self.0)
    }

    /// Сколько кадров в секунду допускает эта длительность; `None` для нулевой (HAL не ограничил).
    pub fn frames_per_second(self) -> Option<f64> {
        if self.0 == 0 {
            return None;
        }
        Some(1e9 / self.0 as f64)
    }
}

/// Один вариант RAW-потока из карты потоков HAL
/// (`ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS` + минимальные длительности кадра).
///
/// `min_frame_duration` — **публично объявленный** потолок. На OnePlus 11 он везде 33,3 мс,
/// хотя реально достижимы 60 к/с (00 C2): объявление — не приговор, а отправная точка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RawStreamOption {
    pub format: RawFormat,
    pub size: PixelSize,
    pub mosaic: MosaicPattern,
    pub min_frame_duration: FrameDuration,
}

/// Диапазон частоты кадров автоэкспозиции (`ACAMERA_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct FpsRange {
    pub min: u32,
    pub max: u32,
}

/// Диапазон светочувствительности (`ACAMERA_SENSOR_INFO_SENSITIVITY_RANGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IsoRange {
    pub min: u32,
    pub max: u32,
}

/// Диапазон выдержки (`ACAMERA_SENSOR_INFO_EXPOSURE_TIME_RANGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ExposureTimeRange {
    pub shortest: Duration,
    pub longest: Duration,
}

/// Ручное управление сенсором: доступно только с capability `MANUAL_SENSOR`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ManualSensorRanges {
    Supported {
        iso: IsoRange,
        exposure_time: ExposureTimeRange,
        /// Самый длинный кадр, который HAL разрешает (`ACAMERA_SENSOR_INFO_MAX_FRAME_DURATION`).
        max_frame_duration: FrameDuration,
    },
    NotSupported,
}

/// Уровни сенсора: чёрный по каналам CFA и белый (насыщение).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SensorLevels {
    /// `ACAMERA_SENSOR_BLACK_LEVEL_PATTERN`: по каналам квадрата 2×2 в порядке строк.
    pub black_level_pattern: [u32; 4],
    /// `ACAMERA_SENSOR_INFO_WHITE_LEVEL`.
    pub white_level: u32,
}

/// Рациональное число из метаданных Camera2 (матрицы цвета хранятся точно, без потери в f32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Rational {
    pub numerator: i32,
    pub denominator: i32,
}

impl Rational {
    /// Значение как f64; `None` при нулевом знаменателе (битые метаданные).
    pub fn to_f64(self) -> Option<f64> {
        if self.denominator == 0 {
            return None;
        }
        Some(f64::from(self.numerator) / f64::from(self.denominator))
    }
}

/// Матрица 3×3 в порядке строк (как в DNG и Camera2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Matrix3x3(pub [Rational; 9]);

/// Эталонный источник света калибровки (коды EXIF LightSource, как в
/// `ACAMERA_SENSOR_REFERENCE_ILLUMINANT1/2` и DNG CalibrationIlluminant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "code")]
pub enum ReferenceIlluminant {
    /// 17 — лампа накаливания, стандартный источник A.
    StandardA,
    /// 21 — дневной свет D65.
    D65,
    /// 23 — D50.
    D50,
    /// Любой другой код EXIF (Camera2 допускает 1–24): храним как есть, не теряем.
    Other(u8),
}

impl ReferenceIlluminant {
    pub fn from_exif_code(code: u8) -> Self {
        match code {
            17 => ReferenceIlluminant::StandardA,
            21 => ReferenceIlluminant::D65,
            23 => ReferenceIlluminant::D50,
            other => ReferenceIlluminant::Other(other),
        }
    }

    pub fn exif_code(self) -> u8 {
        match self {
            ReferenceIlluminant::StandardA => 17,
            ReferenceIlluminant::D65 => 21,
            ReferenceIlluminant::D50 => 23,
            ReferenceIlluminant::Other(code) => code,
        }
    }
}

/// Один набор калибровки цвета под один эталонный свет (как пара тегов DNG …1 / …2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ColorCalibrationSet {
    pub illuminant: ReferenceIlluminant,
    /// XYZ → камера (`ACAMERA_SENSOR_COLOR_TRANSFORM*`, DNG ColorMatrix).
    pub color_matrix: Matrix3x3,
    /// Камера → XYZ D50 (`ACAMERA_SENSOR_FORWARD_MATRIX*`, DNG ForwardMatrix).
    pub forward_matrix: Matrix3x3,
    /// Поправка конкретного экземпляра сенсора (`ACAMERA_SENSOR_CALIBRATION_TRANSFORM*`).
    pub calibration_transform: Matrix3x3,
}

/// Что HAL сообщил о RAW: уровни, раскладка CFA, калибровка цвета.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawSensorInfo {
    pub levels: SensorLevels,
    pub cfa: CfaLayout,
    pub native_mosaic: NativeMosaic,
    /// Обычно два набора (иллюминанты 1 и 2); пусто, если HAL матриц не дал.
    pub color_calibration: Vec<ColorCalibrationSet>,
}

/// Поддержка RAW: только с capability `RAW`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RawSupport {
    Supported(RawSensorInfo),
    NotSupported,
}

/// Насколько честна калибровка расстояния фокуса (`ACAMERA_LENS_INFO_FOCUS_DISTANCE_CALIBRATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusCalibration {
    /// Диоптрии — условные единицы, только порядок «ближе/дальше».
    Uncalibrated,
    /// Примерно в диоптриях.
    Approximate,
    /// Точно в диоптриях.
    Calibrated,
}

/// Фокус: фиксированный (фронт OnePlus 11) или управляемый.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FocusCapability {
    Fixed,
    Adjustable {
        /// Ближайшая дистанция фокуса в диоптриях (`ACAMERA_LENS_INFO_MINIMUM_FOCUS_DISTANCE`).
        min_focus_distance_diopters: f32,
        /// Гиперфокальное расстояние в диоптриях (`ACAMERA_LENS_INFO_HYPERFOCAL_DISTANCE`).
        /// `None` — HAL ключ не отдал (ключ необязательный); значение не подменяется догадкой.
        hyperfocal_distance_diopters: Option<f32>,
        calibration: FocusCalibration,
    },
}

/// Физический размер сенсора в миллиметрах (`ACAMERA_SENSOR_INFO_PHYSICAL_SIZE`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PhysicalSizeMm {
    pub width_mm: f32,
    pub height_mm: f32,
}

/// Оптика: диафрагмы, фокусные расстояния и размер сенсора.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Optics {
    /// Доступные f-числа (`ACAMERA_LENS_INFO_AVAILABLE_APERTURES`).
    pub apertures_f_number: Vec<f32>,
    /// Доступные фокусные расстояния, мм (`ACAMERA_LENS_INFO_AVAILABLE_FOCAL_LENGTHS`).
    pub focal_lengths_mm: Vec<f32>,
    pub sensor_physical_size: PhysicalSizeMm,
}

impl Optics {
    /// Горизонтальный угол обзора в градусах по самому короткому фокусному расстоянию.
    ///
    /// Это угол по длинной стороне сенсора — так он сопоставим между камерами независимо
    /// от того, как сенсор повёрнут в корпусе. `None`, если фокусных расстояний нет
    /// или размеры не положительные (битые метаданные).
    pub fn widest_field_of_view_degrees(&self) -> Option<f64> {
        let shortest_focal_mm = self
            .focal_lengths_mm
            .iter()
            .copied()
            .filter(|focal| focal.is_finite() && *focal > 0.0)
            .reduce(f32::min)?;
        let long_side_mm = self
            .sensor_physical_size
            .width_mm
            .max(self.sensor_physical_size.height_mm);
        if !(long_side_mm.is_finite() && long_side_mm > 0.0) {
            return None;
        }
        let half_angle = (f64::from(long_side_mm) / 2.0 / f64::from(shortest_focal_mm)).atan();
        Some((2.0 * half_angle).to_degrees())
    }
}

/// Применил ли вендор коррекцию виньетирования к RAW (`ACAMERA_SENSOR_INFO_LENS_SHADING_APPLIED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LensShading {
    /// Коррекция уже в пикселях RAW и не отключается (OnePlus 11, 00 C3): наш ISP обязан
    /// учитывать усиленный в углах шум, а не корректировать второй раз.
    AppliedByVendor,
    NotApplied,
    /// HAL ключ не отдал.
    NotReported,
}

/// Вмешательство вендора в пиксели RAW (инвариант 2 README: честно отмечаем, не замалчиваем).
///
/// Здесь — факты, а не настройки: то, что можно было бы отключить, отключается запросом,
/// а то, что не отключается, фиксируется тут.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct VendorProcessingFlags {
    pub lens_shading: LensShading,
}

/// Поддерживаемый уровень HAL (`ACAMERA_INFO_SUPPORTED_HARDWARE_LEVEL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareLevel {
    Legacy,
    Limited,
    Full,
    Level3,
    External,
}

/// Объявленные возможности (`ACAMERA_REQUEST_AVAILABLE_CAPABILITIES`). Нужные нам — по имени,
/// остальные сохраняются кодом, чтобы каталог не терял информацию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "code")]
pub enum RequestCapability {
    BackwardCompatible,
    ManualSensor,
    ManualPostProcessing,
    Raw,
    ReadSensorSettings,
    BurstCapture,
    LogicalMultiCamera,
    /// `ACAMERA_REQUEST_AVAILABLE_CAPABILITIES_SYSTEM_CAMERA`: камера только для системных приложений.
    SystemCamera,
    Other(u8),
}

/// Всё, что движку и продукту нужно знать о камере.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CameraCapabilities {
    pub facing: LensFacing,
    pub topology: CameraTopology,
    pub hardware_level: HardwareLevel,
    pub request_capabilities: Vec<RequestCapability>,
    /// Полный массив пикселей сенсора (`ACAMERA_SENSOR_INFO_PIXEL_ARRAY_SIZE`).
    pub pixel_array: PixelSize,
    /// Активная область (`ACAMERA_SENSOR_INFO_ACTIVE_ARRAY_SIZE`).
    pub active_array: PixelRect,
    /// Поворот кадра сенсора к естественной ориентации телефона (`ACAMERA_SENSOR_ORIENTATION`).
    pub sensor_orientation: SensorOrientation,
    /// Публичные RAW-потоки RAW10/RAW16 (без `RAW_PRIVATE`).
    pub raw_streams: Vec<RawStreamOption>,
    pub ae_fps_ranges: Vec<FpsRange>,
    pub manual_sensor: ManualSensorRanges,
    pub raw: RawSupport,
    pub focus: FocusCapability,
    pub optics: Optics,
    pub vendor_processing: VendorProcessingFlags,
}

impl CameraCapabilities {
    /// Самый большой публичный RAW-поток этого формата (по числу пикселей).
    pub fn largest_raw_stream(&self, format: RawFormat) -> Option<&RawStreamOption> {
        self.raw_streams
            .iter()
            .filter(|option| option.format == format)
            .max_by_key(|option| option.size.pixel_count())
    }

    pub fn has_capability(&self, capability: RequestCapability) -> bool {
        self.request_capabilities.contains(&capability)
    }
}

/// Камера в каталоге: ID и возможности.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CameraDescriptor {
    pub id: CameraId,
    pub capabilities: CameraCapabilities,
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
