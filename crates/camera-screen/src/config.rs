//! Конфиг экрана камеры: все числа поведения и раскладки в одном месте, с метками происхождения
//! (`[doc]` / `[measured]` / `[review]` / `[reverse]` / `[derived]` / `[estimate]`; оценки — «подгонка, не Apple»).
//!
//! Значения по умолчанию документируют источник рядом с полем (в конфиге раздела). Неизвестное поле
//! в JSON — ошибка (`deny_unknown_fields`), невалидное значение — ошибка [`ScreenConfig::validate`].

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::exposure::{ExposureConfig, ExposureConfigError};
use crate::focus::{FocusConfig, FocusConfigError};
use crate::layout::{LayoutConfig, LayoutError};
use crate::level::{LevelConfig, LevelConfigError};
use crate::shutter::{CaptureConfig, CaptureConfigError, TimerConfig, TimerConfigError};
use crate::tools::{ToolId, ToolSet, ToolSetError};
use crate::widgets_config::{WidgetConfigError, WidgetLayoutConfig};
use crate::zoom_config::{ZoomConfig, ZoomConfigError};

/// Наборы инструментов режима Photo как данные (М3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    /// Значки над кадром («Essentials»). `[doc]`/`[estimate]` `behavior.md` 2.2: вспышка, RAW,
    /// таймер (виден, только когда включён), EV.
    pub essentials: Vec<ToolId>,
    /// Состав и порядок меню Controls v1: Flash, Aspect, Timer / Exposure, RAW, Grid / Level.
    /// `[measured: C t=30.0]` порядок iOS 27 без нереализованных Live/Depth/Night; `[estimate]` Grid и Level
    /// стоят в меню до сессии 16 (шестерёнки настроек пока нет, инвариант 6).
    pub controls_menu: Vec<ToolId>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            essentials: vec![ToolId::Flash, ToolId::Raw, ToolId::Timer, ToolId::Exposure],
            controls_menu: vec![
                ToolId::Flash,
                ToolId::Aspect,
                ToolId::Timer,
                ToolId::Exposure,
                ToolId::Raw,
                ToolId::Grid,
                ToolId::Level,
            ],
        }
    }
}

impl ToolsConfig {
    /// Проверенный набор инструментов.
    pub fn tool_set(&self) -> Result<ToolSet, ToolSetError> {
        ToolSet::new(self.essentials.clone(), self.controls_menu.clone())
    }
}

/// Конфиг экрана.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenConfig {
    pub layout: LayoutConfig,
    pub exposure: ExposureConfig,
    pub timer: TimerConfig,
    /// Ожидание итога снимка (аддитивно, пакет P5c).
    pub capture: CaptureConfig,
    pub focus: FocusConfig,
    pub level: LevelConfig,
    pub tools: ToolsConfig,
    /// Раскладка и вид виджетов (аддитивно, пакет P5b).
    pub widgets: WidgetLayoutConfig,
    /// Зум, лента кнопок, колесо, переходы между камерами (сессия 13).
    pub zoom: ZoomConfig,
}

/// Ошибка конфига экрана.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScreenConfigError {
    #[error("раскладка: {0}")]
    Layout(#[from] LayoutError),
    #[error("экспозиция: {0}")]
    Exposure(#[from] ExposureConfigError),
    #[error("таймер: {0}")]
    Timer(#[from] TimerConfigError),
    #[error("снимок: {0}")]
    Capture(#[from] CaptureConfigError),
    #[error("индикатор фокуса: {0}")]
    Focus(#[from] FocusConfigError),
    #[error("уровень: {0}")]
    Level(#[from] LevelConfigError),
    #[error("инструменты: {0}")]
    Tools(#[from] ToolSetError),
    #[error("виджеты: {0}")]
    Widgets(#[from] WidgetConfigError),
    #[error("зум: {0}")]
    Zoom(#[from] ZoomConfigError),
}

impl ScreenConfig {
    /// Проверяет все разделы; первая найденная ошибка возвращается как есть.
    pub fn validate(&self) -> Result<(), ScreenConfigError> {
        self.layout.validate()?;
        self.exposure.validate()?;
        self.timer.validate()?;
        self.capture.validate()?;
        self.focus.validate()?;
        self.level.validate()?;
        self.tools.tool_set()?;
        self.widgets.validate()?;
        self.zoom.validate()?;
        Ok(())
    }
}
