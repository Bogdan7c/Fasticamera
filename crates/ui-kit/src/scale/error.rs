//! Ошибки шкалы.

use thiserror::Error;
use ui_core::scroll::ScrollError;

use crate::drive::DriveError;
use crate::tokens::TokenError;
use crate::value::ValueError;

/// Ошибка создания шкалы или вызова её намерения.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScaleError {
    /// Модель значений отвергла значение или пределы.
    #[error("модель значений: {0}")]
    Value(#[from] ValueError),
    /// Движок прокрутки отверг вызов (например, `drag_moved` без `drag_began`).
    #[error("прокрутка: {0}")]
    Scroll(#[from] ScrollError),
    /// Токены некорректны.
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    /// Магнит на опорных значениях имеет смысл только у плавной модели: у ступенчатой остановка
    /// между шагами дала бы значение, которого не видно на шкале.
    #[error("ContinuousWithMagnets требует плавной модели значений")]
    MagnetsRequireContinuousModel,
}

impl From<DriveError> for ScaleError {
    fn from(error: DriveError) -> Self {
        match error {
            DriveError::Tokens(error) => ScaleError::Tokens(error),
            DriveError::Scroll(error) => ScaleError::Scroll(error),
        }
    }
}
