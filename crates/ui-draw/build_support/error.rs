//! Ошибка запекания: текст с понятным контекстом (имя файла, элемент, символ).
//!
//! Это код сборки, а не продукта: ошибка печатается в stderr, `build.rs` завершается с ненулевым кодом.

use std::fmt;

/// Ошибка запекания шрифта или иконок; всегда содержит человекочитаемое описание причины.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BakeError(pub String);

impl BakeError {
    /// Создаёт ошибку из любого текста.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for BakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BakeError {}

/// Результат операций запекания.
pub type BakeResult<T> = Result<T, BakeError>;
