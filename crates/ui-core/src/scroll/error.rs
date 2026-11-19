//! Ошибки прокрутки.

use thiserror::Error;

use crate::spring::SpringError;

/// Ошибка конфигурации или вызова движка прокрутки. Намеренно различает причины:
/// «движок не в том состоянии» ([`ScrollError::NotDragging`]) не смешивается с плохим числом.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScrollError {
    #[error("доля скорости за 1 мс должна лежать в (0, 1), получено {0}")]
    InvalidDecelerationRate(f64),
    #[error("порог остановки должен быть конечным и > 0, получено {0}")]
    InvalidStopVelocity(f64),
    #[error("коэффициент «резинки» должен быть конечным и > 0, получено {0}")]
    InvalidRubberCoefficient(f64),
    #[error("размер области для «резинки» должен быть конечным и > 0, получено {0}")]
    InvalidViewportExtent(f64),
    #[error("границы прокрутки некорректны: min = {min}, max = {max} (нужно min <= max, оба конечны)")]
    InvalidBounds { min: f64, max: f64 },
    #[error("точка прилипания не конечна: {0}")]
    InvalidSnapPoint(f64),
    #[error("начальное смещение {offset} вне границ [{min}, {max}]")]
    InitialOffsetOutOfBounds { offset: f64, min: f64, max: f64 },
    #[error("значение не конечно: {field} = {value}")]
    NonFiniteInput { field: &'static str, value: f64 },
    #[error("перетаскивание не начато: вызови begin_drag")]
    NotDragging,
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
}
