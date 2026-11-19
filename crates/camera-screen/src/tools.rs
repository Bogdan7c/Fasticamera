//! М3 «инструменты как данные» (`docs/ios-camera/feature-map.md`): идентификаторы инструментов,
//! набор значков над кадром («Essentials») и состав меню Controls.
//!
//! Ряд значков и меню — **данные** (`Vec<ToolId>` из конфига), а не вёрстка «вручную»: так экран
//! «Edit / Reset to Essentials» из iOS 27 потом правит список, а не код виджета.

use thiserror::Error;

use crate::settings::{AspectRatio, FlashMode, GridSetting, LevelSetting, RawCapture, TimerSetting};

/// Инструмент камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolId {
    Flash,
    Raw,
    Timer,
    Exposure,
    Aspect,
    Grid,
    Level,
    /// Место под Live Photos (бэклог). Не реализовано, конфиг его отвергает.
    Live,
    /// Место под Photographic Styles (photo+). Не реализовано.
    Styles,
    /// Место под ночной режим (photo+). Не реализовано.
    Night,
}

impl ToolId {
    /// Реализован ли инструмент в v1. Нереализованные нельзя положить в конфиг.
    pub const fn is_implemented(self) -> bool {
        !matches!(self, ToolId::Live | ToolId::Styles | ToolId::Night)
    }

    /// Может ли инструмент стоять значком над кадром (`behavior.md` 2.2: вспышка, RAW, таймер, EV).
    /// Остальные (соотношение, сетка, уровень) живут только в меню Controls.
    pub const fn can_be_strip_icon(self) -> bool {
        matches!(self, ToolId::Flash | ToolId::Raw | ToolId::Timer | ToolId::Exposure)
    }
}

/// Один вариант в подменю инструмента — «значение + к какому инструменту относится».
///
/// Тип связывает инструмент и его настройку: нельзя выбрать «таймер = 16:9».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOption {
    Flash(FlashMode),
    Raw(RawCapture),
    Timer(TimerSetting),
    Aspect(AspectRatio),
    Grid(GridSetting),
    Level(LevelSetting),
}

impl ToolOption {
    /// Инструмент, чьему подменю принадлежит вариант.
    pub const fn tool(self) -> ToolId {
        match self {
            ToolOption::Flash(_) => ToolId::Flash,
            ToolOption::Raw(_) => ToolId::Raw,
            ToolOption::Timer(_) => ToolId::Timer,
            ToolOption::Aspect(_) => ToolId::Aspect,
            ToolOption::Grid(_) => ToolId::Grid,
            ToolOption::Level(_) => ToolId::Level,
        }
    }
}

/// Почему набор инструментов не принят.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ToolSetError {
    #[error("инструмент {0:?} не реализован в v1 и не может стоять в наборе")]
    NotImplemented(ToolId),
    #[error("инструмент {0:?} повторяется в наборе")]
    Duplicate(ToolId),
    #[error("инструмент {0:?} нельзя ставить значком над кадром")]
    NotAStripIcon(ToolId),
    #[error("меню Controls пусто")]
    EmptyMenu,
}

/// Набор инструментов режима: значки над кадром и состав меню Controls.
///
/// Конструктор проверяет данные — невалидный набор не существует.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSet {
    strip: Vec<ToolId>,
    menu: Vec<ToolId>,
}

impl ToolSet {
    /// Проверенный набор. Значки над кадром могут быть пустыми (пользователь убрал всё), меню — нет.
    pub fn new(strip: Vec<ToolId>, menu: Vec<ToolId>) -> Result<Self, ToolSetError> {
        check_unique_and_implemented(&strip)?;
        check_unique_and_implemented(&menu)?;
        if let Some(tool) = strip.iter().find(|tool| !tool.can_be_strip_icon()) {
            return Err(ToolSetError::NotAStripIcon(*tool));
        }
        if menu.is_empty() {
            return Err(ToolSetError::EmptyMenu);
        }
        Ok(Self { strip, menu })
    }

    /// Значки над кадром в порядке слева направо (до фильтра «таймер виден, только когда включён»).
    pub fn strip(&self) -> &[ToolId] {
        &self.strip
    }

    /// Пункты меню Controls в порядке сетки.
    pub fn menu(&self) -> &[ToolId] {
        &self.menu
    }

    /// Есть ли инструмент в меню Controls.
    pub fn menu_contains(&self, tool: ToolId) -> bool {
        self.menu.contains(&tool)
    }

    /// Есть ли инструмент среди значков над кадром.
    pub fn strip_contains(&self, tool: ToolId) -> bool {
        self.strip.contains(&tool)
    }
}

fn check_unique_and_implemented(tools: &[ToolId]) -> Result<(), ToolSetError> {
    for (index, tool) in tools.iter().enumerate() {
        if !tool.is_implemented() {
            return Err(ToolSetError::NotImplemented(*tool));
        }
        if tools[..index].contains(tool) {
            return Err(ToolSetError::Duplicate(*tool));
        }
    }
    Ok(())
}
