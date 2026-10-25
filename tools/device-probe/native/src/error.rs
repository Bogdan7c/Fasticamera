//! Ошибки нативной части. На границе JNI любая из них превращается в JSON
//! `{"ok":false,"stage":...,"error":...}` (см. `jni_boundary`).

use thiserror::Error;

/// Неожиданный сбой пробы (не «штатный» отрицательный результат вроде отказа драйвера
/// импортировать буфер — тот попадает в отчёт как данные).
#[derive(Debug, Error)]
pub enum ProbeError {
    /// Не удалось сериализовать отчёт.
    #[error("сериализация отчёта: {0}")]
    Serialize(#[from] serde_json::Error),
    /// Сбой NDK-вызова, после которого продолжать нельзя.
    #[error("{0}")]
    Platform(String),
}

impl ProbeError {
    /// Короткий конструктор для платформенных сбоев.
    pub fn platform(message: impl Into<String>) -> Self {
        Self::Platform(message.into())
    }
}
