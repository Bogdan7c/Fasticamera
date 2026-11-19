//! Ошибки плашки.

use thiserror::Error;
use ui_core::spring::SpringError;

use crate::tokens::TokenError;

/// Ошибка создания плашки или вызова её намерения.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum BannerError {
    /// Срок жизни должен быть положительным: нулевой плашка не успела бы появиться.
    #[error("срок жизни плашки должен быть > 0")]
    NonPositiveLifetime,
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
}
