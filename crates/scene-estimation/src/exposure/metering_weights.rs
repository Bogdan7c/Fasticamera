//! Таблица весов плиток замера AE: считается на CPU один раз на поток и область, дальше —
//! плоский массив f32 (готов к загрузке в буфер шейдера).
//!
//! Две области (решение 68):
//! * **Матричный** — центро-взвешенный гаусс по осям активной области сенсора;
//! * **Точечный** — вес плитки равен доле её площади, закрытой регионом тапа, плюс малый вес
//!   фона, чтобы пустая от ячеек область не обнуляла замер целиком.
//!
//! Нормировка: сумма весов ровно 1 (в f64, затем сужение до f32), поэтому замер — взвешенное
//! среднее без отдельного деления.

use camera_contract::three_a::MeteringRegion;
use isp_model::statistics::TileGrid;

use super::config::MeteringConfig;
use crate::error::ConfigError;

/// Где замеряем.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeteringArea {
    /// Весь кадр, центр тяжелее.
    Matrix,
    /// Регион тапа (оси активной области сенсора).
    Spot(MeteringRegion),
}

/// Почему таблицу весов построить не удалось.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeteringWeightsError {
    /// Конфиг замера неверен.
    #[error("конфиг замера: {0}")]
    Config(#[from] ConfigError),
    /// Сетка без плиток.
    #[error("сетка {columns}×{rows} пуста")]
    EmptyGrid {
        /// Столбцов.
        columns: u32,
        /// Строк.
        rows: u32,
    },
    /// Прямоугольник сетки неверен (вне [0, 1]², перевёрнут, NaN) или соотношение сторон не положительно.
    #[error("положение сетки неверно: нужны 0 <= left < right <= 1, 0 <= top < bottom <= 1 и aspect > 0")]
    InvalidPlacement,
    /// Регион точечного замера не пересекает ни одной плитки: замерять нечего.
    #[error("регион точечного замера лежит вне сетки статистики")]
    RegionOutsideGrid,
}

/// Где лежит сетка статистики в нормированных осях активной области сенсора (те же оси, что
/// у [`MeteringRegion`]): прямоугольник `[left, right] × [top, bottom]` внутри `[0, 1]²`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TilePlacement {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    active_aspect_ratio: f32,
}

impl TilePlacement {
    /// Сетка покрывает прямоугольник; `active_aspect_ratio` — ширина / высота активной области
    /// сенсора в пикселях. Нужно, чтобы квадратный (в пикселях) регион тапа стал верным
    /// прямоугольником в нормированных осях.
    pub fn new(
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        active_aspect_ratio: f32,
    ) -> Result<Self, MeteringWeightsError> {
        let in_unit =
            |low: f32, high: f32| low.is_finite() && high.is_finite() && 0.0 <= low && low < high && high <= 1.0;
        if !(in_unit(left, right)
            && in_unit(top, bottom)
            && active_aspect_ratio.is_finite()
            && active_aspect_ratio > 0.0)
        {
            return Err(MeteringWeightsError::InvalidPlacement);
        }
        Ok(Self {
            left,
            top,
            right,
            bottom,
            active_aspect_ratio,
        })
    }

    /// Сетка покрывает всю активную область.
    pub fn full_frame(active_aspect_ratio: f32) -> Result<Self, MeteringWeightsError> {
        Self::new(0.0, 0.0, 1.0, 1.0, active_aspect_ratio)
    }
}

/// Прямоугольник в нормированных осях.
#[derive(Debug, Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Rect {
    fn area(self) -> f64 {
        (self.right - self.left) * (self.bottom - self.top)
    }

    /// Площадь пересечения с другим прямоугольником (0, если не пересекаются).
    fn overlap_area(self, other: Rect) -> f64 {
        let width = (self.right.min(other.right) - self.left.max(other.left)).max(0.0);
        let height = (self.bottom.min(other.bottom) - self.top.max(other.top)).max(0.0);
        width * height
    }
}

/// Нормированные веса плиток по строкам сетки (`index = row · columns + column`), сумма = 1.
#[derive(Debug, Clone, PartialEq)]
pub struct MeteringWeights {
    grid: TileGrid,
    weights: Vec<f32>,
    highlight_protection: bool,
}

impl MeteringWeights {
    /// Строит таблицу для сетки, её положения и области замера.
    pub fn build(
        grid: TileGrid,
        placement: TilePlacement,
        area: MeteringArea,
        config: &MeteringConfig,
    ) -> Result<Self, MeteringWeightsError> {
        config.validate()?;
        if grid.columns == 0 || grid.rows == 0 {
            return Err(MeteringWeightsError::EmptyGrid {
                columns: grid.columns,
                rows: grid.rows,
            });
        }
        let raw = match area {
            MeteringArea::Matrix => matrix_weights(grid, placement, config),
            MeteringArea::Spot(region) => spot_weights(grid, placement, region, config)?,
        };
        Ok(Self {
            grid,
            weights: normalize(&raw),
            // Защита светов нужна, когда в замер попадает вся сцена; тап пользователя — осознанный выбор.
            highlight_protection: matches!(area, MeteringArea::Matrix),
        })
    }

    /// Сетка, под которую построена таблица.
    pub fn grid(&self) -> TileGrid {
        self.grid
    }

    /// Веса плиток по строкам сетки.
    pub fn weights(&self) -> &[f32] {
        &self.weights
    }

    /// Применять ли защиту светов (только матричный замер).
    pub fn applies_highlight_protection(&self) -> bool {
        self.highlight_protection
    }
}

/// Прямоугольник плитки `(column, row)` в нормированных осях активной области.
fn tile_rect(grid: TileGrid, placement: TilePlacement, column: u32, row: u32) -> Rect {
    let width = f64::from(placement.right - placement.left) / f64::from(grid.columns);
    let height = f64::from(placement.bottom - placement.top) / f64::from(grid.rows);
    let left = f64::from(placement.left) + width * f64::from(column);
    let top = f64::from(placement.top) + height * f64::from(row);
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// Гаусс от расстояния центра плитки до центра кадра, в долях кадра по каждой оси.
fn matrix_weights(grid: TileGrid, placement: TilePlacement, config: &MeteringConfig) -> Vec<f64> {
    let two_sigma_squared = 2.0 * f64::from(config.matrix_center_sigma).powi(2);
    each_tile(grid, |column, row| {
        let tile = tile_rect(grid, placement, column, row);
        let dx = (tile.left + tile.right) / 2.0 - 0.5;
        let dy = (tile.top + tile.bottom) / 2.0 - 0.5;
        (-(dx * dx + dy * dy) / two_sigma_squared).exp()
    })
}

/// Доля площади плитки под регионом + малый фон; пустое пересечение со всей сеткой — ошибка.
fn spot_weights(
    grid: TileGrid,
    placement: TilePlacement,
    region: MeteringRegion,
    config: &MeteringConfig,
) -> Result<Vec<f64>, MeteringWeightsError> {
    // Сторона региона — доля МЕНЬШЕЙ стороны активной области; пересчёт в оси [0,1]²:
    // при ширине больше высоты квадрат уже по x, иначе уже по y.
    let aspect = f64::from(placement.active_aspect_ratio);
    let side_in_height_units = f64::from(region.size_fraction()) * aspect.min(1.0);
    let half_x = side_in_height_units / aspect / 2.0;
    let half_y = side_in_height_units / 2.0;
    let center_x = f64::from(region.center().x());
    let center_y = f64::from(region.center().y());
    let region_rect = Rect {
        left: center_x - half_x,
        top: center_y - half_y,
        right: center_x + half_x,
        bottom: center_y + half_y,
    };
    let background = f64::from(config.spot_background_weight);
    let mut any_overlap = false;
    let weights = each_tile(grid, |column, row| {
        let tile = tile_rect(grid, placement, column, row);
        let covered = tile.overlap_area(region_rect) / tile.area();
        any_overlap |= covered > 0.0;
        covered + (1.0 - covered) * background
    });
    if any_overlap {
        Ok(weights)
    } else {
        Err(MeteringWeightsError::RegionOutsideGrid)
    }
}

/// Значения для всех плиток по строкам.
fn each_tile(grid: TileGrid, mut weight_of: impl FnMut(u32, u32) -> f64) -> Vec<f64> {
    let mut weights = Vec::with_capacity(usize::try_from(grid.tile_count()).unwrap_or(0));
    for row in 0..grid.rows {
        for column in 0..grid.columns {
            weights.push(weight_of(column, row));
        }
    }
    weights
}

/// Нормировка суммы к 1 в f64 и сужение до f32.
fn normalize(raw: &[f64]) -> Vec<f32> {
    let total: f64 = raw.iter().sum();
    raw.iter().map(|weight| (weight / total) as f32).collect()
}

#[cfg(test)]
#[path = "metering_weights_tests.rs"]
mod tests;
