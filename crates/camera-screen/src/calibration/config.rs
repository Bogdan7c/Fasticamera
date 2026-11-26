//! Конфиг раздела калибровки: флаг режима, раскладка зон (`layout_config`), стандарт вектороскопа по умолчанию, предел
//! картинки прибора.

use scope_contract::{MAX_SCOPE_IMAGE_SIDE_PX, VectorscopeAxes};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::layout_config::{CalibrationLayoutConfig, CalibrationLayoutConfigError};
use super::widgets_config::{CalibrationWidgetsConfig, CalibrationWidgetsConfigError};
use crate::mode_behaviour::CalibrationAvailability;

/// Конфиг раздела калибровки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CalibrationConfig {
    /// Стоит ли режим CALIBRATE в карусели (решение 185). По умолчанию включён.
    pub enabled: bool,
    /// Стандарт осей вектороскопа при запуске; меняется переключателем в полоске прибора.
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
    #[error("раскладка раздела: {0}")]
    Layout(#[from] CalibrationLayoutConfigError),
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
        self.layout.validate()?;
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
