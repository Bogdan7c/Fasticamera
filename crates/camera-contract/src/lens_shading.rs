//! Калибровка виньетирования (затенения объектива): словарь для паспорта потока и ISP.
//!
//! # Зачем
//! Флаг `SENSOR_INFO_LENS_SHADING_APPLIED = true` у OnePlus 11 означает лишь ЧАСТИЧНУЮ коррекцию
//! (документация Camera2): углы RAW остаются на 2,2–3 EV темнее центра. Поэтому мы корректируем
//! сами по собственной статичной калибровке камеры (яркость и цвет). Этот модуль — только данные
//! и их проверка: интерполяцию и применение к пикселям держит `isp-model`.
//!
//! # Соглашение сетки (то же, что у карты `LENS_SHADING_MAP` в Camera2)
//! * точка `(c, r)` лежит в пикселе активной области
//!   `((W − 1)/(columns − 1)·c, (H − 1)/(rows − 1)·r)`, крайние точки — точно в углах;
//! * точки идут по строкам, в каждой четыре усиления `[R, G_even, G_odd, B]`;
//! * `G_even` — зелёный чётных строк АКТИВНОЙ ОБЛАСТИ, `G_odd` — нечётных;
//! * все усиления ≥ 1: коррекция только поднимает тёмные места до яркости самого светлого.

use std::fmt;
use std::sync::Arc;

use serde::{Serialize, Serializer};

use crate::geometry::{PixelRect, PixelSize};
use crate::ids::CameraId;

/// Число каналов в точке сетки: `[R, G_even, G_odd, B]`.
pub const GAIN_CHANNELS: usize = 4;
/// Минимальная сетка: билинейной интерполяции нужны хотя бы две точки по каждой оси.
const MIN_GRID_POINTS: u32 = 2;

/// Сетку усилений нельзя построить.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidGainGrid {
    #[error("сетка усилений {columns}×{rows} меньше 2×2")]
    GridTooSmall { columns: u32, rows: u32 },
    #[error("сетка {columns}×{rows} требует {expected} точек, передано {actual}")]
    PointCountMismatch {
        columns: u32,
        rows: u32,
        expected: usize,
        actual: usize,
    },
    #[error("усиление {gain} в точке {point} не конечно или меньше 1")]
    InvalidGain { point: usize, gain: f32 },
}

/// Сетка усилений виньетирования: данные и их инварианты (см. модуль).
#[derive(Clone, PartialEq)]
pub struct LensShadingGainGrid {
    columns: u32,
    rows: u32,
    /// Точки в порядке строк, в каждой `[R, G_even, G_odd, B]`.
    points: Vec<[f32; GAIN_CHANNELS]>,
}

impl LensShadingGainGrid {
    /// Проверяет размер сетки, число точек и значения усилений (конечные и ≥ 1).
    pub fn new(columns: u32, rows: u32, points: Vec<[f32; GAIN_CHANNELS]>) -> Result<Self, InvalidGainGrid> {
        if columns < MIN_GRID_POINTS || rows < MIN_GRID_POINTS {
            return Err(InvalidGainGrid::GridTooSmall { columns, rows });
        }
        let expected = columns as usize * rows as usize;
        if points.len() != expected {
            return Err(InvalidGainGrid::PointCountMismatch {
                columns,
                rows,
                expected,
                actual: points.len(),
            });
        }
        let invalid = points.iter().enumerate().find_map(|(point, gains)| {
            gains
                .iter()
                // `is_finite` отсекает NaN и бесконечности, затем проверка «меньше 1».
                .find(|gain| !gain.is_finite() || **gain < 1.0)
                .map(|&gain| (point, gain))
        });
        if let Some((point, gain)) = invalid {
            return Err(InvalidGainGrid::InvalidGain { point, gain });
        }
        Ok(Self { columns, rows, points })
    }

    pub fn columns(&self) -> u32 {
        self.columns
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Точки по строкам, в каждой `[R, G_even, G_odd, B]`.
    pub fn points(&self) -> &[[f32; GAIN_CHANNELS]] {
        &self.points
    }

    /// Сетка, где каждое усиление приближено к 1: `g' = 1 + (g − 1)·weight`.
    ///
    /// Вес вне `0…1` зажимается, NaN считается нулём, поэтому результат всегда сохраняет
    /// инвариант «усиление конечно и ≥ 1».
    pub fn blended_toward_unity(&self, weight: f32) -> Self {
        let weight = if weight.is_nan() { 0.0 } else { weight.clamp(0.0, 1.0) };
        Self {
            columns: self.columns,
            rows: self.rows,
            points: self
                .points
                .iter()
                .map(|gains| gains.map(|gain| 1.0 + (gain - 1.0) * weight))
                .collect(),
        }
    }

    /// Усиления точки `(column, row)`; `None` — точки за сеткой нет.
    pub fn point(&self, column: u32, row: u32) -> Option<[f32; GAIN_CHANNELS]> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        self.points
            .get(row as usize * self.columns as usize + column as usize)
            .copied()
    }
}

/// Компактный вывод: 33×25 точек по четыре числа в логах не нужны.
impl fmt::Debug for LensShadingGainGrid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "LensShadingGainGrid({}×{})", self.columns, self.rows)
    }
}

/// Статичная калибровка виньетирования одной камеры.
#[derive(Debug, Clone, PartialEq)]
pub struct LensShadingCalibration {
    pub camera: CameraId,
    /// Размер активной области, для которого снята сетка (проверяется против камеры).
    pub active_array: PixelSize,
    pub grid: LensShadingGainGrid,
}

/// Где полезная область буфера лежит на активной области сенсора — единственный владелец
/// геометрии «поток → сенсор» (калибровка и привязка к кадру читают её отсюда).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ValidAreaOnSensor {
    /// Размер активной области сенсора, в пикселях.
    pub active_array: PixelSize,
    /// Прямоугольник полезной области буфера в пикселях активной области.
    /// Размер может отличаться от размера полезной области буфера: буфер меньше активной
    /// области при биннинге (тогда соответствие масштабированием, см. `isp-model`).
    pub rect: PixelRect,
}

/// Прямоугольник вне активной области или пуст.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("прямоугольник {rect:?} пуст или не помещается в активную область {active_array:?}")]
pub struct InvalidSensorArea {
    pub active_array: PixelSize,
    pub rect: PixelRect,
}

impl ValidAreaOnSensor {
    /// Полезная область — вся активная область (4:3 основной камеры OnePlus 11).
    pub const fn whole_active_array(active_array: PixelSize) -> Self {
        Self {
            active_array,
            rect: PixelRect {
                left: 0,
                top: 0,
                size: active_array,
            },
        }
    }

    /// Проверяет, что `rect` непуст и лежит внутри активной области.
    pub fn new(active_array: PixelSize, rect: PixelRect) -> Result<Self, InvalidSensorArea> {
        let right = u64::from(rect.left) + u64::from(rect.size.width);
        let bottom = u64::from(rect.top) + u64::from(rect.size.height);
        let fits = rect.size.width > 0
            && rect.size.height > 0
            && right <= u64::from(active_array.width)
            && bottom <= u64::from(active_array.height);
        if !fits {
            return Err(InvalidSensorArea { active_array, rect });
        }
        Ok(Self { active_array, rect })
    }
}

/// Почему у потока нет нашей калибровки виньетирования.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum NotCalibratedReason {
    /// В профиле устройства нет калибровки для этой камеры.
    NoCalibrationForCamera,
    /// Калибровка снята для другой активной области, чем у камеры: сетке верить нельзя.
    ActiveArrayMismatch { calibration: PixelSize, camera: PixelSize },
    /// Профиль калибровок не относится к этому устройству (другая модель телефона).
    DeviceNotInProfile,
}

/// Есть ли у потока наша калибровка виньетирования.
///
/// Флаг `vendor_processing.lens_shading` («вендор частично скорректировал») нашу стадию не
/// управляет: решает только это состояние.
#[derive(Clone)]
pub enum LensShadingCalibrationState {
    Calibrated(Arc<LensShadingCalibration>),
    NotCalibrated(NotCalibratedReason),
}

impl LensShadingCalibrationState {
    /// Калибровка, если она есть.
    pub fn calibration(&self) -> Option<&Arc<LensShadingCalibration>> {
        match self {
            Self::Calibrated(calibration) => Some(calibration),
            Self::NotCalibrated(_) => None,
        }
    }
}

/// Равенство нужно ключам кэша параметров: сначала дешёвое сравнение указателей `Arc`,
/// затем по значению (две загрузки одного файла дают равные калибровки).
impl PartialEq for LensShadingCalibrationState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Calibrated(left), Self::Calibrated(right)) => Arc::ptr_eq(left, right) || left == right,
            (Self::NotCalibrated(left), Self::NotCalibrated(right)) => left == right,
            _ => false,
        }
    }
}

impl fmt::Debug for LensShadingCalibrationState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Calibrated(calibration) => write!(
                formatter,
                "Calibrated(camera {}, active {}×{}, grid {}×{})",
                calibration.camera,
                calibration.active_array.width,
                calibration.active_array.height,
                calibration.grid.columns(),
                calibration.grid.rows()
            ),
            Self::NotCalibrated(reason) => write!(formatter, "NotCalibrated({reason:?})"),
        }
    }
}

/// В отчётах — краткая сводка без самих точек.
impl Serialize for LensShadingCalibrationState {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case", tag = "state")]
        enum Summary<'a> {
            Calibrated {
                camera: &'a CameraId,
                active_array: PixelSize,
                grid_columns: u32,
                grid_rows: u32,
            },
            NotCalibrated {
                #[serde(flatten)]
                reason: NotCalibratedReason,
            },
        }
        match self {
            Self::Calibrated(calibration) => Summary::Calibrated {
                camera: &calibration.camera,
                active_array: calibration.active_array,
                grid_columns: calibration.grid.columns(),
                grid_rows: calibration.grid.rows(),
            },
            Self::NotCalibrated(reason) => Summary::NotCalibrated { reason: *reason },
        }
        .serialize(serializer)
    }
}

/// Набор калибровок всех камер устройства: приходит из профиля устройства (не из Kotlin-конфига)
/// и раздаётся бэкенду при построении паспортов потоков.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LensShadingCalibrations {
    calibrations: Vec<Arc<LensShadingCalibration>>,
    /// Профиль калибровок вообще не относится к этому устройству: у каждой камеры причина —
    /// [`NotCalibratedReason::DeviceNotInProfile`], а не «для этой камеры нет».
    device_not_in_profile: bool,
}

impl LensShadingCalibrations {
    /// Пустой набор: калибровок нет ни у одной камеры.
    pub fn none() -> Self {
        Self::default()
    }

    /// Набор для устройства, чей профиль калибровок есть.
    pub fn new(calibrations: Vec<LensShadingCalibration>) -> Self {
        Self {
            calibrations: calibrations.into_iter().map(Arc::new).collect(),
            device_not_in_profile: false,
        }
    }

    /// Профиль калибровок относится к другому устройству.
    pub fn device_not_in_profile() -> Self {
        Self {
            calibrations: Vec::new(),
            device_not_in_profile: true,
        }
    }

    /// Камеры, для которых калибровка есть.
    pub fn cameras(&self) -> impl Iterator<Item = &CameraId> {
        self.calibrations.iter().map(|calibration| &calibration.camera)
    }

    pub fn is_empty(&self) -> bool {
        self.calibrations.is_empty()
    }

    /// Состояние калибровки потока камеры `camera` с активной областью `active_array`.
    pub fn state_for(&self, camera: &CameraId, active_array: PixelSize) -> LensShadingCalibrationState {
        if self.device_not_in_profile {
            return LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::DeviceNotInProfile);
        }
        let Some(calibration) = self.calibrations.iter().find(|candidate| &candidate.camera == camera) else {
            return LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::NoCalibrationForCamera);
        };
        if calibration.active_array != active_array {
            return LensShadingCalibrationState::NotCalibrated(NotCalibratedReason::ActiveArrayMismatch {
                calibration: calibration.active_array,
                camera: active_array,
            });
        }
        LensShadingCalibrationState::Calibrated(Arc::clone(calibration))
    }
}

#[cfg(test)]
#[path = "lens_shading_tests.rs"]
mod tests;
