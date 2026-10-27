//! Стадия 1: уровни чёрного и белого → нормированная мозаика.
//!
//! Формула (спецификация DNG, глава 5 «Mapping Raw Values to Linear Reference Values»,
//! шаги BlackLevel и WhiteLevel):
//!
//! ```text
//! linear = max(raw − black[позиция 2×2], 0) / (white − black[позиция 2×2])
//! ```
//!
//! - Чёрный уровень — свой у каждой позиции квадрата 2×2 (Camera2 `BLACK_LEVEL_PATTERN`).
//! - Отрицательные значения (шум вокруг чёрного) обрезаются до 0. На OnePlus 11 шум у чёрного
//!   около 0,4 единицы RAW (профиль шума C7), поэтому смещение среднего в тенях пренебрежимо,
//!   а демозаик и тон не получают отрицательный свет.
//! - Сверху значение не обрезается: обрезку пересвета делает баланс белого, где известен
//!   уровень насыщения после усилений (см. [`crate::white_balance`]).

use camera_contract::capabilities::SensorLevels;

use crate::cfa::CfaCell;
use crate::image::{CameraMosaic, RawMosaic, mosaic_stage};

/// Уровни нарушают физический смысл: белый не выше чёрного или значения не конечны.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidLevels {
    #[error("белый уровень {white} не выше чёрного {black} в позиции {cell} квадрата 2×2")]
    WhiteNotAboveBlack { cell: usize, black: f32, white: f32 },
    #[error("уровень не конечен или отрицателен: чёрный {black:?}, белый {white}")]
    NotFiniteOrNegative { black: [f32; 4], white: f32 },
}

/// Уровни нормировки кадра: чёрный по позициям квадрата 2×2 и общий белый (в единицах RAW).
///
/// Уровни дробные: динамический чёрный уровень Camera2 (`SENSOR_DYNAMIC_BLACK_LEVEL`) — float.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizationLevels {
    black: [f32; 4],
    white: f32,
}

impl NormalizationLevels {
    pub fn new(black: [f32; 4], white: f32) -> Result<Self, InvalidLevels> {
        let all_valid = black
            .iter()
            .chain([&white])
            .all(|level| level.is_finite() && *level >= 0.0);
        if !all_valid {
            return Err(InvalidLevels::NotFiniteOrNegative { black, white });
        }
        if let Some((cell, &cell_black)) = black.iter().enumerate().find(|(_, cell_black)| white <= **cell_black) {
            return Err(InvalidLevels::WhiteNotAboveBlack {
                cell,
                black: cell_black,
                white,
            });
        }
        Ok(Self { black, white })
    }

    /// Статические уровни сенсора из характеристик камеры.
    pub fn from_sensor_levels(levels: &SensorLevels) -> Result<Self, InvalidLevels> {
        // u32 → f32 без потерь: уровни RAW ≤ 2^16.
        Self::new(
            levels.black_level_pattern.map(|level| level as f32),
            levels.white_level as f32,
        )
    }

    /// Чёрный уровень позиции квадрата 2×2.
    pub fn black(&self, cell: CfaCell) -> f32 {
        self.black[cell.index()]
    }

    pub fn white(&self) -> f32 {
        self.white
    }

    /// Чёрные уровни всех позиций в порядке Camera2.
    pub fn black_pattern(&self) -> [f32; 4] {
        self.black
    }
}

/// Нормирует один отсчёт RAW по уровням его позиции (формула модуля).
pub fn normalize_sample(raw: u16, cell: CfaCell, levels: &NormalizationLevels) -> f32 {
    let black = levels.black(cell);
    let range = levels.white() - black;
    (f32::from(raw) - black).max(0.0) / range
}

/// Стадия 1: RAW u16 → нормированная мозаика (чёрный 0, белый сенсора 1).
pub fn normalize_levels(raw: &RawMosaic, levels: &NormalizationLevels) -> CameraMosaic {
    raw.map_into(mosaic_stage::Normalized, |x, y, sample| {
        normalize_sample(sample, CfaCell::of_pixel(x, y), levels)
    })
}

#[cfg(test)]
#[path = "levels_tests.rs"]
mod tests;
