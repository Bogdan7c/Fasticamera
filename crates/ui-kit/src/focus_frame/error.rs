//! Ошибки рамки фокуса.

use thiserror::Error;
use ui_core::geometry::Point;
use ui_core::spring::SpringError;

use crate::drive::GrowthDirection;
use crate::scale::ScaleError;
use crate::tokens::TokenError;

/// Ошибка создания рамки или вызова её намерения.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FocusFrameError {
    /// «Солнце» ходит вверх-вниз: горизонтальная ось не подходит.
    #[error("«солнце» должно быть вертикальным, получено {0:?}")]
    SunMustBeVertical(GrowthDirection),
    /// У «солнца» нет рисок: оно лишь значок на шкале без рисования.
    #[error("«солнце» не рисует риски: ожидалось TickVisibility::Hidden")]
    SunTicksMustBeHidden,
    /// Сроки удержания: `0 < dim_after < hide_after`.
    #[error("сроки удержания рамки должны удовлетворять 0 < dim_after < hide_after")]
    InvalidHold,
    /// Точка фокуса не число.
    #[error("точка фокуса должна быть конечной: {0:?}")]
    InvalidPoint(Point),
    /// Жест пришёл на скрытую рамку: экран не должен подписывать «солнце», пока рамки нет.
    #[error("рамка фокуса скрыта: жест на «солнце» невозможен")]
    NotVisible,
    #[error("шкала «солнца»: {0}")]
    Scale(#[from] ScaleError),
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
}
