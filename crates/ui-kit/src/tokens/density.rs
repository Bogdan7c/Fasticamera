//! Токены плотности шкал и раскладки каруселей.
//!
//! Модель значений измеряет положение в своих «единицах» (шаг для линейной и ряда, 1/N октавы
//! для логарифмической). Сколько pt занимает единица на экране, решает токен плотности:
//! так одна и та же модель может быть плотной линейкой или редким колесом.

use serde::{Deserialize, Serialize};

use super::error::TokenError;
use super::points_per_unit::PointsPerUnit;

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
    /// Линейка экспозиции нижней карточки: одна единица модели (шаг 1/3 EV) — 20 pt.
    Ruler,
    /// Дуговое колесо (зум): одна единица модели (октава) — длина дуги по кольцу рисок.
    Dial,
    /// Плотность задана числом в самом описании шкалы, а не токеном: так шкале, чьё число определяет экран
    /// (линейка ручек раздела калибровки: ход пальца и число единиц лежат в конфиге экрана, решение 251),
    /// не нужен глобальный токен на каждую такую линейку.
    Explicit(PointsPerUnit),
    /// Непрерывная линейка Кельвинов ([`crate::value::MiredKelvin`]): одна единица модели — один майред.
    /// Отдельный токен: подгоняется независимо от шкалы оттенка.
    MiredRuler,
    /// Непрерывная линейка с равномерной осью ([`crate::value::ContinuousLinear`], оттенок ±50): одна единица
    /// модели — одна единица значения.
    TintRuler,
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
    /// pt на единицу при [`ScaleDensity::Ruler`]: единица модели — один шаг 1/3 EV.
    /// `[measured: R00 t=35.17–35.90]`: 20,0.
    pub ruler_points_per_unit: f64,
    /// pt на единицу при [`ScaleDensity::Dial`]: единица модели — октава зума, её длина на кольце рисок.
    /// `[derived: R00 t=88.5–100]`: 14,2° на октаву × радиус кольца ≈ 230 pt ≈ 57 pt.
    pub dial_points_per_unit: f64,
    /// pt на единицу при [`ScaleDensity::MiredRuler`]: один майред. `[estimate]` 2 — риска каждые 5 майред
    /// (10 pt), решение WB34 плана white-balance; подгоняется на телефоне.
    pub mired_ruler_points_per_unit: f64,
    /// pt на единицу при [`ScaleDensity::TintRuler`]: одна единица оттенка. `[estimate]` 2 — риска каждые
    /// 5 единиц (10 pt), решение WB34; подгоняется на телефоне.
    pub tint_ruler_points_per_unit: f64,
    /// Радиус «магнита» опорного значения у непрерывной шкалы, pt. `[estimate]` — подгонка, не Apple.
    pub magnet_capture_points: f64,
    /// Радиус магнита у дугового колеса ([`ScaleDensity::Dial`]), pt. `[estimate]` 6 — подгонка, не Apple: при
    /// 57 pt на октаву это ≈ ±0,1 октавы; общие 14 pt залипали бы на 2× уже от 1,7×.
    pub dial_magnet_capture_points: f64,
    /// Промежуток между элементами карусели, pt. `[measured]` `layout.md`: зазор между словами 35,4.
    pub carousel_item_gap_points: f64,
    /// На сколько pt за рамку кнопки палец ещё считается «на кнопке» (зона отслеживания нажатия).
    /// `[reverse]` 8: Android `ViewConfiguration.TOUCH_SLOP` = 8 dp (AOSP `ViewConfiguration.java`,
    /// `View.onTouchEvent` снимает «нажато», когда палец ушёл за рамку больше чем на slop). Число
    /// UIKit в документации не найдено. Подгонка, не Apple (`behavior.md` 1.1, «кнопка»).
    pub press_tracking_slop_points: f64,
}

impl Default for DensityTokens {
    fn default() -> Self {
        Self {
            dense_points_per_unit: 12.0,
            regular_points_per_unit: 24.0,
            sparse_points_per_unit: 48.0,
            ruler_points_per_unit: 20.0,
            dial_points_per_unit: 57.0,
            mired_ruler_points_per_unit: 2.0,
            tint_ruler_points_per_unit: 2.0,
            magnet_capture_points: 14.0,
            dial_magnet_capture_points: 6.0,
            carousel_item_gap_points: 35.4,
            press_tracking_slop_points: 8.0,
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
            ScaleDensity::Ruler => self.ruler_points_per_unit,
            ScaleDensity::Dial => self.dial_points_per_unit,
            ScaleDensity::Explicit(points_per_unit) => points_per_unit.get(),
            ScaleDensity::MiredRuler => self.mired_ruler_points_per_unit,
            ScaleDensity::TintRuler => self.tint_ruler_points_per_unit,
        }
    }

    /// Радиус магнита для шкалы данной плотности, pt.
    pub fn magnet_capture(&self, density: ScaleDensity) -> f64 {
        match density {
            ScaleDensity::Dial => self.dial_magnet_capture_points,
            ScaleDensity::Dense
            | ScaleDensity::Regular
            | ScaleDensity::Sparse
            | ScaleDensity::Ruler
            | ScaleDensity::Explicit(_)
            | ScaleDensity::MiredRuler
            | ScaleDensity::TintRuler => self.magnet_capture_points,
        }
    }

    /// Все размеры конечны и > 0.
    pub fn validate(&self) -> Result<(), TokenError> {
        let sizes = [
            ("dense_points_per_unit", self.dense_points_per_unit),
            ("regular_points_per_unit", self.regular_points_per_unit),
            ("sparse_points_per_unit", self.sparse_points_per_unit),
            ("ruler_points_per_unit", self.ruler_points_per_unit),
            ("dial_points_per_unit", self.dial_points_per_unit),
            ("mired_ruler_points_per_unit", self.mired_ruler_points_per_unit),
            ("tint_ruler_points_per_unit", self.tint_ruler_points_per_unit),
            ("magnet_capture_points", self.magnet_capture_points),
            ("dial_magnet_capture_points", self.dial_magnet_capture_points),
            ("carousel_item_gap_points", self.carousel_item_gap_points),
            ("press_tracking_slop_points", self.press_tracking_slop_points),
        ];
        for (name, value) in sizes {
            if !value.is_finite() || value <= 0.0 {
                return Err(TokenError::NonPositiveSize { name, value });
            }
        }
        Ok(())
    }
}
