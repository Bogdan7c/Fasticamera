//! Конфиг раздела калибровки: флаг режима, доли зон раскладки, стандарт вектороскопа по умолчанию, предел
//! картинки прибора. Числа раскладки — `[estimate]` (решение 202: «доли — конфиг»), не измерение Apple:
//! в эталоне нет ни кадра раздела калибровки, ни аналога (правило 96).

use scope_contract::{MAX_SCOPE_IMAGE_SIDE_PX, VectorscopeAxes};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::widgets_config::{CalibrationWidgetsConfig, CalibrationWidgetsConfigError};
use crate::mode_behaviour::CalibrationAvailability;

/// Доли двух разрезов раскладки в «глазных» координатах одной ориентации.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoneFractions {
    /// Какую долю ширины занимает колонка ручек слева (0..1, не включая края).
    pub knobs_width_fraction: f64,
    /// Какую долю высоты правой колонки (над нижним рядом) занимает чистое превью справа сверху; остальное — прибор.
    pub preview_height_fraction: f64,
}

/// Числа раскладки раздела.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationLayoutConfig {
    /// Портрет: ручки слева на половину ширины (эскиз решения 185), справа сверху ≈ четверть экрана чистого
    /// превью (доля подобрана так, чтобы с нижним рядом получилось ≈ 0,5 высоты экрана) `[estimate]`.
    pub portrait: ZoneFractions,
    /// Альбом (решение 202): колонка ручек ≈ треть ширины, правая колонка поровну между превью и прибором `[estimate]`.
    pub landscape: ZoneFractions,
    /// Высота нижнего ряда в портрете, pt. Ряд занимает то место, где в PHOTO стоят кнопка меню (Ø44 на линии
    /// затвора) и капсула режимов: от верха кнопки меню с запасом до низа экрана `[estimate]` ≈ 956 − 770.
    pub portrait_bottom_row_height_pt: f64,
    /// Высота нижнего ряда в альбоме, pt: в альбоме экран по высоте всего 440 pt, поэтому кнопка меню и капсула
    /// стоят в одной компактной линии `[estimate]`.
    pub landscape_bottom_row_height_pt: f64,
}

impl Default for CalibrationLayoutConfig {
    fn default() -> Self {
        Self {
            portrait: ZoneFractions {
                knobs_width_fraction: 0.5,
                preview_height_fraction: 0.62,
            },
            landscape: ZoneFractions {
                knobs_width_fraction: 1.0 / 3.0,
                preview_height_fraction: 0.5,
            },
            portrait_bottom_row_height_pt: 186.0,
            landscape_bottom_row_height_pt: 64.0,
        }
    }
}

/// Конфиг раздела калибровки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationConfig {
    /// Стоит ли режим CALIBRATE в карусели (решение 185). По умолчанию включён.
    pub enabled: bool,
    /// Стандарт осей вектороскопа при запуске; меняется в меню раздела.
    pub vectorscope_axes: VectorscopeAxes,
    /// Предел стороны картинки прибора в пикселях (рычаг бюджета; не больше предела словаря
    /// [`MAX_SCOPE_IMAGE_SIDE_PX`]). Зона больше предела просит картинку по пределу.
    pub max_scope_image_side_px: u32,
    /// Раскладка зон.
    pub layout: CalibrationLayoutConfig,
    /// Числа вида виджетов раздела (колонка ручек, строки, прибор).
    pub widgets: CalibrationWidgetsConfig,
}

impl Default for CalibrationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            // Y'CbCr BT.709 — стандарт видеомонтажа (решение 185); OKLab — выбор в меню.
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            max_scope_image_side_px: MAX_SCOPE_IMAGE_SIDE_PX,
            layout: CalibrationLayoutConfig::default(),
            widgets: CalibrationWidgetsConfig::default(),
        }
    }
}

/// Ошибка конфига раздела калибровки.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CalibrationConfigError {
    #[error("доля {field} должна быть числом строго между 0 и 1: {value}")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("высота нижнего ряда {field} должна быть конечной и больше нуля: {value}")]
    InvalidBottomRowHeight { field: &'static str, value: f64 },
    #[error("предел картинки прибора {value} px вне 1..={max}")]
    ScopeSideLimitOutOfRange { value: u32, max: u32 },
    #[error("виджеты раздела: {0}")]
    Widgets(#[from] CalibrationWidgetsConfigError),
}

impl CalibrationConfig {
    /// Режим калибровки в терминах реестра режимов.
    pub fn availability(&self) -> CalibrationAvailability {
        if self.enabled {
            CalibrationAvailability::Enabled
        } else {
            CalibrationAvailability::Disabled
        }
    }

    /// Проверка значений (не зависит от размера surface).
    pub fn validate(&self) -> Result<(), CalibrationConfigError> {
        let fractions = [
            (
                "portrait.knobs_width_fraction",
                self.layout.portrait.knobs_width_fraction,
            ),
            (
                "portrait.preview_height_fraction",
                self.layout.portrait.preview_height_fraction,
            ),
            (
                "landscape.knobs_width_fraction",
                self.layout.landscape.knobs_width_fraction,
            ),
            (
                "landscape.preview_height_fraction",
                self.layout.landscape.preview_height_fraction,
            ),
        ];
        for (field, value) in fractions {
            if !(value.is_finite() && value > 0.0 && value < 1.0) {
                return Err(CalibrationConfigError::FractionOutOfRange { field, value });
            }
        }
        let heights = [
            (
                "portrait_bottom_row_height_pt",
                self.layout.portrait_bottom_row_height_pt,
            ),
            (
                "landscape_bottom_row_height_pt",
                self.layout.landscape_bottom_row_height_pt,
            ),
        ];
        for (field, value) in heights {
            if !(value.is_finite() && value > 0.0) {
                return Err(CalibrationConfigError::InvalidBottomRowHeight { field, value });
            }
        }
        if self.max_scope_image_side_px == 0 || self.max_scope_image_side_px > MAX_SCOPE_IMAGE_SIDE_PX {
            return Err(CalibrationConfigError::ScopeSideLimitOutOfRange {
                value: self.max_scope_image_side_px,
                max: MAX_SCOPE_IMAGE_SIDE_PX,
            });
        }
        self.widgets.validate()?;
        Ok(())
    }
}
