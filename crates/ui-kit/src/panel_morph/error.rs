//! Ошибки панели-морфа.

use thiserror::Error;
use ui_core::geometry::Rect;
use ui_core::spring::SpringError;

use crate::tokens::TokenError;

/// Ошибка создания панели или вызова её намерения.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum PanelError {
    /// Рамка источника или цели вырождена (не число или нулевой размер).
    #[error("рамка «{which}» вырождена: {frame:?}")]
    InvalidFrame { which: &'static str, frame: Rect },
    /// Радиус скругления цели не число или отрицателен.
    #[error("радиус скругления цели должен быть конечным и >= 0, получено {0}")]
    InvalidRadius(f64),
    /// Расстояние въезда содержимого не число или отрицательно.
    #[error("расстояние въезда содержимого должно быть конечным и >= 0, получено {0}")]
    InvalidContentSlide(f64),
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
}
