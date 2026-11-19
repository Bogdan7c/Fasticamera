//! Типы правил вёрстки (данные без логики решения).

use thiserror::Error;

use crate::geometry::Size;
use crate::id::NodeId;
use crate::tree::TreeError;

/// От какой области родителя отсчитываются отступы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReferenceArea {
    /// Вся рамка родителя.
    #[default]
    Frame,
    /// Безопасная зона родителя (рамка минус его `safe_insets`).
    SafeArea,
}

/// Размещение по одной оси относительно опорной области. Длина — размер элемента по оси.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AxisPlacement {
    /// К началу оси (левый край / верх) с отступом.
    Start { inset: f64, length: f64 },
    /// К концу оси (правый край / низ) с отступом — основа привязки к низу.
    End { inset: f64, length: f64 },
    /// По центру области со сдвигом (положительный — к концу оси).
    Center { offset: f64, length: f64 },
    /// Растянуть между краями с отступами; длина — остаток.
    Stretch { start_inset: f64, end_inset: f64 },
}

/// Правило якоря: область + размещение по каждой оси.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorRule {
    pub area: ReferenceArea,
    pub horizontal: AxisPlacement,
    pub vertical: AxisPlacement,
}

impl AnchorRule {
    /// Заполнить всю рамку родителя.
    pub const fn fill_parent() -> Self {
        let stretch = AxisPlacement::Stretch {
            start_inset: 0.0,
            end_inset: 0.0,
        };
        Self {
            area: ReferenceArea::Frame,
            horizontal: stretch,
            vertical: stretch,
        }
    }
}

/// Как элемент размещается у родителя.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// По якорю (родитель — свободный контейнер).
    Anchored(AnchorRule),
    /// Место в стеке родителя с фиксированным размером.
    InStack(Size),
}

impl Default for Placement {
    fn default() -> Self {
        Placement::Anchored(AnchorRule::fill_parent())
    }
}

/// Ось главного направления стека.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAxis {
    Horizontal,
    Vertical,
}

/// Выравнивание детей по поперечной оси стека.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossAlignment {
    Start,
    Center,
    End,
}

/// Расположение связки детей по главной оси стека.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainDistribution {
    Start,
    Center,
    End,
}

/// Параметры стека. Размер детей — их фиксированный размер ([`Placement::InStack`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackSpec {
    pub axis: StackAxis,
    /// Промежуток между соседними детьми, pt (>= 0).
    pub spacing: f64,
    pub cross_alignment: CrossAlignment,
    pub distribution: MainDistribution,
    /// В какой области родителя раскладывать детей.
    pub area: ReferenceArea,
}

/// Как элемент размещает своих детей.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ContainerLayout {
    /// Дети размещаются по своим якорям.
    #[default]
    Free,
    /// Дети выстраиваются стеком.
    Stack(StackSpec),
}

/// Ошибка вёрстки. Рамки при ошибке не меняются.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum LayoutError {
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// Размер/длина отрицательны или не число.
    #[error("недопустимая длина {value} pt у элемента {node:?}: нужно конечное число >= 0")]
    InvalidLength { node: NodeId, value: f64 },
    /// Отступ, сдвиг или промежуток — не число (или промежуток отрицателен).
    #[error("недопустимый отступ/промежуток {value} у элемента {node:?}")]
    InvalidOffset { node: NodeId, value: f64 },
    /// Растяжение даёт отрицательную длину: отступы больше области.
    #[error("растяжение элемента {node:?} даёт отрицательную длину {resulting_length} pt")]
    NegativeStretch { node: NodeId, resulting_length: f64 },
    /// Тип размещения ребёнка не подходит контейнеру родителя.
    #[error("размещение элемента {node:?} не подходит способу раскладки его родителя")]
    PlacementMismatch { node: NodeId },
}
