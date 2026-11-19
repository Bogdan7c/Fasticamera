//! Ошибки карусели.

use thiserror::Error;
use ui_core::scroll::ScrollError;

use crate::drive::DriveError;
use crate::tokens::TokenError;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum CarouselError {
    /// Карусель без элементов не существует.
    #[error("список элементов карусели пуст")]
    EmptyEntries,
    #[error("ширина элемента {index} должна быть конечной и > 0, получено {width_points}")]
    InvalidWidth { index: usize, width_points: f64 },
    /// Такого элемента нет в списке.
    #[error("элемента нет в списке карусели")]
    UnknownItem,
    #[error("индекс элемента {index} вне списка из {length}")]
    IndexOutOfRange { index: usize, length: usize },
    #[error("прокрутка: {0}")]
    Scroll(#[from] ScrollError),
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
}

impl From<DriveError> for CarouselError {
    fn from(error: DriveError) -> Self {
        match error {
            DriveError::Tokens(error) => CarouselError::Tokens(error),
            DriveError::Scroll(error) => CarouselError::Scroll(error),
        }
    }
}
