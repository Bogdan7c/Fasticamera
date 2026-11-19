//! Ошибки рендера UI. Различаем: ошибка настроек, неверное использование (слот, цель), плохой список
//! рисования, превышение предела кадра и сбой GPU — вызывающему важна разница.

use ash::vk;
use gpu_vulkan::error::GpuError;
use thiserror::Error;
use ui_draw::draw_list::DrawListError;

use crate::config::ConfigError;

/// Какой предел кадра превышен.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Закрашенные фигуры.
    ShapeInstances,
    /// Глифы текста и иконки.
    GlyphInstances,
    /// Группы стекла.
    GlassGroups,
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ShapeInstances => "фигур",
            Self::GlyphInstances => "глифов и иконок",
            Self::GlassGroups => "групп стекла",
        })
    }
}

/// Ошибка рендера UI. Во всех вариантах, кроме `Gpu`, команды в буфер ещё НЕ записаны и цель не тронута.
#[derive(Debug, Error)]
pub enum UiRenderError {
    /// Настройки неверны.
    #[error("настройки рендера UI: {0}")]
    Config(#[from] ConfigError),
    /// Слот кадра вне 0..frame_slots.
    #[error("слот кадра {slot} вне 0..{slots}")]
    SlotOutOfRange {
        /// Запрошенный слот.
        slot: usize,
        /// Сколько слотов.
        slots: usize,
    },
    /// Слот ещё не освобождён `release_slot_after_fence`: GPU может читать его буферы.
    #[error("слот кадра {slot} ещё в полёте: вызовите release_slot_after_fence после fence")]
    SlotInFlight {
        /// Слот.
        slot: usize,
    },
    /// Формат цели не равен формату настроек.
    #[error("формат цели {actual:?}, рендер создан под {expected:?}")]
    TargetFormatMismatch {
        /// Из настроек.
        expected: vk::Format,
        /// У цели.
        actual: vk::Format,
    },
    /// Размер цели не соответствует логическому размеру и повороту.
    #[error("размер цели {actual:?}, а логический размер с поворотом даёт {expected:?}")]
    TargetSizeMismatch {
        /// Ожидаемый физический размер.
        expected: vk::Extent2D,
        /// У цели.
        actual: vk::Extent2D,
    },
    /// Логический размер не конечен или нулевой.
    #[error("логический размер {width}x{height} непригоден")]
    InvalidLogicalSize {
        /// Ширина.
        width: f32,
        /// Высота.
        height: f32,
    },
    /// Зеркальный preTransform не поддержан.
    #[error("зеркальный поворот цели не поддержан: {0}")]
    UnsupportedSurfaceTransform(GpuError),
    /// Список рисования не прошёл проверку словаря.
    #[error("список рисования отклонён: {0}")]
    InvalidDrawList(#[from] DrawListError),
    /// Превышен предел кадра: ничего не обрезано молча, кадр не нарисован.
    #[error("в кадре {requested} {kind}, предел {limit}")]
    LimitExceeded {
        /// Что превышено.
        kind: LimitKind,
        /// Сколько в кадре.
        requested: usize,
        /// Предел из настроек.
        limit: u32,
    },
    /// Сбой GPU-набора (создание объектов, запись команд). Командный буфер после него годится только на выброс.
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
}
