//! Ошибки проверки токенов.

use thiserror::Error;
use ui_core::scroll::ScrollError;
use ui_core::spring::SpringError;
use ui_draw::glass::GlassStyleError;

/// Токен задан неверно. Проверяется один раз в [`super::UiKitTokens::validate`], а не на каждом кадре.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TokenError {
    /// Пружина с таким именем не строится (нефинитные числа, длительность <= 0, bounce вне диапазона).
    #[error("пружина «{name}» некорректна: {source}")]
    InvalidSpring { name: &'static str, source: SpringError },
    /// Токен плотности или размера должен быть конечным и строго положительным.
    #[error("токен «{name}» должен быть конечным и > 0, получено {value}")]
    NonPositiveSize { name: &'static str, value: f64 },
    /// Пресет стекла не прошёл проверку.
    #[error("пресет стекла «{role}» некорректен: {source}")]
    InvalidGlass {
        role: &'static str,
        source: GlassStyleError,
    },
    /// Число токена вне допустимого диапазона (доля вне 0..1, отрицательная длительность и т. п.).
    #[error("токен «{name}» вне допустимого диапазона: {value}")]
    OutOfRange { name: &'static str, value: f64 },
    /// Цвет токена не прошёл проверку (компоненты должны быть конечны и лежать в 0..1).
    #[error("цвет «{name}» некорректен: компоненты должны лежать в 0..1")]
    InvalidColor { name: &'static str },
    /// Базовый конфиг прокрутки `ui-core` не прошёл проверку.
    #[error("конфиг прокрутки некорректен: {0}")]
    Scroll(#[from] ScrollError),
}
