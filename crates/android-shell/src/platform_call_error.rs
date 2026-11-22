//! Итог синхронного вызова Kotlin-сервиса снимка (хранилище, кодеры) с потока записи снимка.
//!
//! Нейтральная часть (тестируется на ПК): android-модули превращают любой сбой JNI в
//! [`PlatformCallError`], а здесь он однозначно переводится в ошибки трейтов `shell-contract`.
//! Так «платформа не подключена» и «платформа отказала» не сливаются в одну строку.

use shell_contract::still_media::{EncodeError, MediaStoreError};

/// Почему вызов Kotlin-сервиса не дал результата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformCallError {
    /// Activity не подключена (или JVM неизвестна): вызывать некого.
    NotAttached,
    /// Kotlin бросил исключение или JNI вернул ошибку; внутри — её текст.
    Failed(String),
}

impl From<PlatformCallError> for MediaStoreError {
    fn from(error: PlatformCallError) -> Self {
        match error {
            PlatformCallError::NotAttached => MediaStoreError::NotAttached,
            PlatformCallError::Failed(reason) => MediaStoreError::Platform(reason),
        }
    }
}

impl From<PlatformCallError> for EncodeError {
    fn from(error: PlatformCallError) -> Self {
        match error {
            PlatformCallError::NotAttached => EncodeError::NotAttached,
            PlatformCallError::Failed(reason) => EncodeError::Platform(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_attached_maps_to_not_attached_in_both_contracts() {
        assert_eq!(
            MediaStoreError::from(PlatformCallError::NotAttached),
            MediaStoreError::NotAttached
        );
        assert_eq!(
            EncodeError::from(PlatformCallError::NotAttached),
            EncodeError::NotAttached
        );
    }

    #[test]
    fn failure_text_is_kept_not_swallowed() {
        let failure = PlatformCallError::Failed("java.lang.SecurityException: нет доступа".to_owned());
        assert_eq!(
            MediaStoreError::from(failure.clone()),
            MediaStoreError::Platform("java.lang.SecurityException: нет доступа".to_owned())
        );
        assert_eq!(
            EncodeError::from(failure),
            EncodeError::Platform("java.lang.SecurityException: нет доступа".to_owned())
        );
    }
}
