//! Ошибка проверки конфигов crate (AE и источник белого «телефон»).

/// Конфиг не прошёл проверку.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// Значение поля вне допустимого диапазона.
    #[error("поле {field}: {reason}")]
    Invalid {
        /// Путь поля в конфиге.
        field: &'static str,
        /// Что не так.
        reason: String,
    },
}
