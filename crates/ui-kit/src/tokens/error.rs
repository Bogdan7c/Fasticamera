//! Ошибки проверки токенов.

use thiserror::Error;
use ui_core::scroll::ScrollError;
use ui_core::spring::SpringError;

/// Токен задан неверно. Проверяется один раз в [`super::UiKitTokens::validate`], а не на каждом кадре.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TokenError {
    /// Пружина с таким именем не строится (нефинитные числа, длительность <= 0, bounce вне диапазона).
    #[error("пружина «{name}» некорректна: {source}")]
    InvalidSpring { name: &'static str, source: SpringError },
    /// Токен плотности или размера должен быть конечным и строго положительным.
    #[error("токен «{name}» должен быть конечным и > 0, получено {value}")]
    NonPositiveSize { name: &'static str, value: f64 },
    /// Базовый конфиг прокрутки `ui-core` не прошёл проверку.
    #[error("конфиг прокрутки некорректен: {0}")]
    Scroll(#[from] ScrollError),
}
