//! Ошибки затвора.

use thiserror::Error;
use ui_core::spring::SpringError;

use crate::button::ButtonError;
use crate::tokens::TokenError;

/// Ошибка создания затвора или обработки нажатия.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ShutterError {
    #[error("нажатие: {0}")]
    Button(#[from] ButtonError),
    #[error("токены: {0}")]
    Tokens(#[from] TokenError),
    #[error("пружина: {0}")]
    Spring(#[from] SpringError),
}
