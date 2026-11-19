//! Анимируемые свойства элемента и работа с ними в дереве.

use crate::geometry::Vector;
use crate::id::NodeId;
use crate::tree::{ElementScale, TreeError, UiTree};

/// Свойство элемента, которое умеет ехать пружиной (всё, что есть в дереве как визуальное).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnimatedProperty {
    /// Прозрачность, `[0, 1]`.
    Opacity,
    /// Масштаб по горизонтали.
    ScaleX,
    /// Масштаб по вертикали.
    ScaleY,
    /// Поворот, радианы.
    Rotation,
    /// Визуальный сдвиг по горизонтали, pt.
    TranslationX,
    /// Визуальный сдвиг по вертикали, pt.
    TranslationY,
}

/// Ключ анимации: «этот элемент, это свойство». Порядок ключей (`Ord`) задаёт детерминированный
/// порядок применения: сначала по элементу, затем по свойству.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropertyKey {
    pub node: NodeId,
    pub property: AnimatedProperty,
}

/// Текущее значение свойства в дереве.
pub(super) fn read_property(tree: &UiTree, key: PropertyKey) -> Result<f64, TreeError> {
    match key.property {
        AnimatedProperty::Opacity => tree.opacity(key.node),
        AnimatedProperty::ScaleX => tree.scale(key.node).map(|scale| scale.x),
        AnimatedProperty::ScaleY => tree.scale(key.node).map(|scale| scale.y),
        AnimatedProperty::Rotation => tree.rotation_radians(key.node),
        AnimatedProperty::TranslationX => tree.translation(key.node).map(|shift| shift.dx),
        AnimatedProperty::TranslationY => tree.translation(key.node).map(|shift| shift.dy),
    }
}

/// Записывает значение в дерево (дерево само проверяет допустимость).
pub(super) fn write_property(tree: &mut UiTree, key: PropertyKey, value: f64) -> Result<(), TreeError> {
    match key.property {
        AnimatedProperty::Opacity => tree.set_opacity(key.node, value),
        AnimatedProperty::ScaleX => {
            let current = tree.scale(key.node)?;
            tree.set_scale(key.node, ElementScale { x: value, ..current })
        }
        AnimatedProperty::ScaleY => {
            let current = tree.scale(key.node)?;
            tree.set_scale(key.node, ElementScale { y: value, ..current })
        }
        AnimatedProperty::Rotation => tree.set_rotation_radians(key.node, value),
        AnimatedProperty::TranslationX => {
            let current = tree.translation(key.node)?;
            tree.set_translation(key.node, Vector { dx: value, ..current })
        }
        AnimatedProperty::TranslationY => {
            let current = tree.translation(key.node)?;
            tree.set_translation(key.node, Vector { dy: value, ..current })
        }
    }
}

/// Допустимый диапазон значений свойства: `[low, high]`, `None` у границы — без ограничения.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ValueRange {
    low: Option<f64>,
    high: Option<f64>,
}

impl ValueRange {
    pub(super) fn for_property(property: AnimatedProperty, min_scale: f64) -> Self {
        match property {
            AnimatedProperty::Opacity => Self {
                low: Some(0.0),
                high: Some(1.0),
            },
            AnimatedProperty::ScaleX | AnimatedProperty::ScaleY => Self {
                low: Some(min_scale),
                high: None,
            },
            // Поворот и сдвиг не ограничены (только конечность).
            AnimatedProperty::Rotation | AnimatedProperty::TranslationX | AnimatedProperty::TranslationY => {
                Self { low: None, high: None }
            }
        }
    }

    /// Значение внутри диапазона? Нечисло — нет.
    pub(super) fn contains(&self, value: f64) -> bool {
        value.is_finite() && self.low.is_none_or(|low| value >= low) && self.high.is_none_or(|high| value <= high)
    }

    /// Значение, прижатое к диапазону.
    pub(super) fn clamp(&self, value: f64) -> f64 {
        let lowered = self.low.map_or(value, |low| value.max(low));
        self.high.map_or(lowered, |high| lowered.min(high))
    }
}
