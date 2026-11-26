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
    /// Ширина магнита нуля — не конечное неотрицательное число.
    #[error("ширина магнита нуля должна быть конечной и не меньше нуля, получено {half_width_points}")]
    InvalidZeroMagnetWidth { half_width_points: f64 },
    /// Магнит нуля держит ступень нуля: у плавной модели нет ступени, а её щелчок опорного значения
    /// рождается по-другому (см. `zero_magnet`).
    #[error("магнит нуля требует ступенчатой модели значений")]
    ZeroMagnetRequiresSteppedModel,
}

impl From<DriveError> for ScaleError {
    fn from(error: DriveError) -> Self {
        match error {
            DriveError::Tokens(error) => ScaleError::Tokens(error),
            DriveError::Scroll(error) => ScaleError::Scroll(error),
        }
    }
}
