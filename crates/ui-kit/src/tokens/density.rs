//! Токены плотности шкал и раскладки каруселей.
//!
//! Модель значений измеряет положение в своих «единицах» (шаг для линейной и ряда, 1/N октавы
//! для логарифмической). Сколько pt занимает единица на экране, решает токен плотности:
//! так одна и та же модель может быть плотной линейкой или редким колесом.

use serde::{Deserialize, Serialize};

use super::error::TokenError;

/// Плотность шкалы: сколько pt занимает одна единица модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleDensity {
    /// Частые тики (линейка с шагом в треть).
    Dense,
    /// Обычная плотность (колёса).
    Regular,
    /// Редкие тики (крупные шаги).
    Sparse,
}

/// Числа раскладки элементов, pt.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DensityTokens {
    /// pt на единицу модели при [`ScaleDensity::Dense`]. `[estimate]` — подгонка, не Apple.
    pub dense_points_per_unit: f64,
    /// pt на единицу при [`ScaleDensity::Regular`]. `[estimate]` — подгонка, не Apple.
    pub regular_points_per_unit: f64,
    /// pt на единицу при [`ScaleDensity::Sparse`]. `[estimate]` — подгонка, не Apple.
    pub sparse_points_per_unit: f64,
    /// Радиус «магнита» опорного значения у непрерывной шкалы, pt. `[estimate]` — подгонка, не Apple.
    pub magnet_capture_points: f64,
    /// Промежуток между элементами карусели, pt. `[measured]` `layout.md`: зазор между словами 35,4.
    pub carousel_item_gap_points: f64,
}

impl Default for DensityTokens {
    fn default() -> Self {
        Self {
            dense_points_per_unit: 12.0,
            regular_points_per_unit: 24.0,
            sparse_points_per_unit: 48.0,
            magnet_capture_points: 14.0,
            carousel_item_gap_points: 35.4,
        }
    }
}

impl DensityTokens {
    /// pt на единицу модели для плотности.
    pub fn points_per_unit(&self, density: ScaleDensity) -> f64 {
        match density {
            ScaleDensity::Dense => self.dense_points_per_unit,
            ScaleDensity::Regular => self.regular_points_per_unit,
            ScaleDensity::Sparse => self.sparse_points_per_unit,
        }
    }

    /// Все размеры конечны и > 0.
    pub fn validate(&self) -> Result<(), TokenError> {
        let sizes = [
            ("dense_points_per_unit", self.dense_points_per_unit),
            ("regular_points_per_unit", self.regular_points_per_unit),
            ("sparse_points_per_unit", self.sparse_points_per_unit),
            ("magnet_capture_points", self.magnet_capture_points),
            ("carousel_item_gap_points", self.carousel_item_gap_points),
        ];
        for (name, value) in sizes {
            if !value.is_finite() || value <= 0.0 {
                return Err(TokenError::NonPositiveSize { name, value });
            }
        }
        Ok(())
    }
}
