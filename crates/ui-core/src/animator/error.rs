//! Ошибки аниматора свойств.

use thiserror::Error;

use super::property::AnimatedProperty;
use crate::spring::SpringError;
use crate::tree::TreeError;

/// Ошибка операции аниматора. Старт анимации при ошибке ничего не меняет.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum AnimatorError {
    /// Элемент не существует (удалён или идентификатор устарел) либо дерево отвергло значение.
    #[error(transparent)]
    Tree(#[from] TreeError),
    /// Пружина или критерий покоя некорректны.
    #[error(transparent)]
    Spring(#[from] SpringError),
    /// Цель вне допустимого диапазона свойства. Перелёт пружины ошибкой не считается — он обрезается
    /// при применении; ошибка только у заведомо неверной цели.
    #[error("цель {value} недопустима для свойства {property:?}")]
    TargetOutOfRange { property: AnimatedProperty, value: f64 },
    /// Минимальный масштаб в конфиге должен быть конечным и > 0.
    #[error("минимальный масштаб должен быть конечным и > 0, получено {0}")]
    InvalidMinScale(f64),
}
