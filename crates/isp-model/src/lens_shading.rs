//! Стадия 2: коррекция виньетирования картой усилений — только если вендор её не применил.
//!
//! На OnePlus 11 коррекция уже в пикселях RAW и не отключается (00 C3), поэтому для наших
//! камер стадия честно пропускается с причиной [`LensShadingSkipReason::VendorAlreadyApplied`].
//! Повторная коррекция подняла бы углы ещё в ~4,3 раза (инвариант 2 README).
//!
//! Карта — как `ACAMERA_STATISTICS_LENS_SHADING_MAP` в Camera2: сетка `columns × rows`,
//! в каждой точке четыре усиления `[R, G_even, G_odd, B]` (G_even — зелёный чётных строк
//! мозаики). Точки сетки равномерно покрывают кадр, крайние лежат точно в углах; между
//! точками усиление интерполируется билинейно (документация `android.statistics.lensShadingMap`).

use camera_contract::capabilities::LensShading;

use crate::cfa::RgbChannel;
use crate::image::{CameraMosaic, ShadingCorrectedMosaic, mosaic_stage};

/// Число каналов в точке карты Camera2: `[R, G_even, G_odd, B]`.
const MAP_CHANNELS: usize = 4;
/// Минимальная сетка: билинейной интерполяции нужны хотя бы две точки по каждой оси.
const MIN_GRID_POINTS: u32 = 2;

/// Карта нельзя применить: сетка мала, данных не столько, сколько точек, или усиление не > 0.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidGainMap {
    #[error("сетка карты {columns}×{rows} меньше 2×2")]
    GridTooSmall { columns: u32, rows: u32 },
    #[error("карта {columns}×{rows} требует {expected} точек, передано {actual}")]
    PointCountMismatch {
        columns: u32,
        rows: u32,
        expected: usize,
        actual: usize,
    },
    #[error("усиление {gain} в точке {point} не конечно или не положительно")]
    InvalidGain { point: usize, gain: f32 },
}

/// Карта усилений виньетирования (см. документацию модуля).
#[derive(Debug, Clone, PartialEq)]
pub struct LensShadingGainMap {
    columns: u32,
    rows: u32,
    /// Точки в порядке строк, в каждой `[R, G_even, G_odd, B]`.
    points: Vec<[f32; MAP_CHANNELS]>,
}

impl LensShadingGainMap {
    pub fn new(columns: u32, rows: u32, points: Vec<[f32; MAP_CHANNELS]>) -> Result<Self, InvalidGainMap> {
        if columns < MIN_GRID_POINTS || rows < MIN_GRID_POINTS {
            return Err(InvalidGainMap::GridTooSmall { columns, rows });
        }
        let expected = columns as usize * rows as usize;
        if points.len() != expected {
            return Err(InvalidGainMap::PointCountMismatch {
                columns,
                rows,
                expected,
                actual: points.len(),
            });
        }
        let invalid = points.iter().enumerate().find_map(|(point, gains)| {
            gains
                .iter()
                .find(|gain| !gain.is_finite() || **gain <= 0.0)
                .map(|&gain| (point, gain))
        });
        if let Some((point, gain)) = invalid {
            return Err(InvalidGainMap::InvalidGain { point, gain });
        }
        Ok(Self { columns, rows, points })
    }

    /// Усиление для пикселя (x, y) кадра `width × height` в канале карты `map_channel`.
    fn gain_at(&self, x: u32, y: u32, width: u32, height: u32, map_channel: usize) -> f32 {
        // Положение пикселя на сетке: крайние пиксели кадра совпадают с крайними точками.
        let grid_x = grid_coordinate(x, width, self.columns);
        let grid_y = grid_coordinate(y, height, self.rows);
        let left = (grid_x.floor() as u32).min(self.columns - 2);
        let top = (grid_y.floor() as u32).min(self.rows - 2);
        let fraction_x = grid_x - left as f32;
        let fraction_y = grid_y - top as f32;
        let point = |column: u32, row: u32| self.points[(row * self.columns + column) as usize][map_channel];
        let upper = point(left, top) * (1.0 - fraction_x) + point(left + 1, top) * fraction_x;
        let lower = point(left, top + 1) * (1.0 - fraction_x) + point(left + 1, top + 1) * fraction_x;
        upper * (1.0 - fraction_y) + lower * fraction_y
    }
}

/// Координата пикселя на сетке из `points` точек: 0 — первая точка, `points − 1` — последняя.
fn grid_coordinate(pixel: u32, extent: u32, points: u32) -> f32 {
    if extent <= 1 {
        return 0.0;
    }
    pixel as f32 / (extent - 1) as f32 * (points - 1) as f32
}

/// Канал карты Camera2 для цвета мозаики в строке `y`.
fn map_channel(channel: RgbChannel, y: u32) -> usize {
    match channel {
        RgbChannel::Red => 0,
        // G_even — зелёный чётных строк мозаики, G_odd — нечётных.
        RgbChannel::Green if y.is_multiple_of(2) => 1,
        RgbChannel::Green => 2,
        RgbChannel::Blue => 3,
    }
}

/// Почему стадия пропущена: различаем «вендор уже сделал» и «нечем делать».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingSkipReason {
    /// Коррекция уже в RAW (OnePlus 11): второй раз нельзя.
    VendorAlreadyApplied,
    /// Вендор не применил, но карты нет: углы останутся тёмными.
    NoGainMap,
}

/// Решение стадии на кадр.
#[derive(Debug, Clone, PartialEq)]
pub enum LensShadingStep {
    Skip(LensShadingSkipReason),
    Apply(LensShadingGainMap),
}

impl LensShadingStep {
    /// Решает по флагу вендора из паспорта и наличию карты.
    ///
    /// «HAL не сообщил» трактуется как «не применено»: двойная коррекция видна сразу (светлые
    /// углы), а пропущенная молча — нет; при этом без карты стадия всё равно пропускается.
    pub fn decide(vendor: LensShading, map: Option<LensShadingGainMap>) -> Self {
        match (vendor, map) {
            (LensShading::AppliedByVendor, _) => Self::Skip(LensShadingSkipReason::VendorAlreadyApplied),
            (LensShading::NotApplied | LensShading::NotReported, Some(map)) => Self::Apply(map),
            (LensShading::NotApplied | LensShading::NotReported, None) => Self::Skip(LensShadingSkipReason::NoGainMap),
        }
    }
}

/// Что стадия сделала с кадром (для диагностики и паритета).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensShadingOutcome {
    Skipped(LensShadingSkipReason),
    Applied,
}

/// Стадия 2: `pixel · gain(x, y, канал)` или пропуск с причиной.
pub fn apply_lens_shading(
    mosaic: &CameraMosaic,
    step: &LensShadingStep,
) -> (ShadingCorrectedMosaic, LensShadingOutcome) {
    match step {
        LensShadingStep::Skip(reason) => (
            mosaic.map_into(mosaic_stage::ShadingCorrected, |_, _, sample| sample),
            LensShadingOutcome::Skipped(*reason),
        ),
        LensShadingStep::Apply(map) => {
            let size = mosaic.size();
            let pattern = mosaic.pattern();
            let corrected = mosaic.map_into(mosaic_stage::ShadingCorrected, |x, y, sample| {
                let channel = map_channel(pattern.channel_at(x, y), y);
                sample * map.gain_at(x, y, size.width, size.height, channel)
            });
            (corrected, LensShadingOutcome::Applied)
        }
    }
}

#[cfg(test)]
#[path = "lens_shading_tests.rs"]
mod tests;
