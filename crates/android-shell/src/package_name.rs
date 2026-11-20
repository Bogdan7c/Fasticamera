//! Имя пакета приложения — факт ПРОЦЕССА, а не окна (решение 106, сессия 12b).
//!
//! Android выдаёт менеджер датчиков только по имени пакета (`ASensorManager_getInstanceForPackage`). Раньше
//! имя приходило в Rust вместе с подключением Activity, а экран камеры включал датчики на долю миллисекунды
//! раньше — и при обычном запуске уровень и поворот значков не работали вовсе (в smoke-сценариях экран ставился
//! позже, поэтому там всё было зелёным). Теперь Kotlin передаёт имя при `initialize`, оболочка хранит его на
//! процесс, и датчики от подключения окна не зависят.
//!
//! Тип нейтральный (без JNI), поэтому проверка значения тестируется на ПК.

use std::ffi::{CStr, CString};

/// Имя пакета, годное для C API: непустое и без нулевых байтов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageName(CString);

/// Имя пакета не годится для C API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidPackageName {
    /// Пустая строка: Android по ней менеджер датчиков не выдаст.
    #[error("имя пакета пустое")]
    Empty,
    /// В строке нулевой байт: C-строка оборвалась бы на нём.
    #[error("в имени пакета нулевой байт на позиции {position}")]
    InteriorNul {
        /// Где именно нулевой байт.
        position: usize,
    },
}

impl PackageName {
    /// Проверенное имя из строки Kotlin (`Context.getPackageName()`).
    pub fn new(name: String) -> Result<Self, InvalidPackageName> {
        if name.is_empty() {
            return Err(InvalidPackageName::Empty);
        }
        CString::new(name)
            .map(Self)
            .map_err(|error| InvalidPackageName::InteriorNul {
                position: error.nul_position(),
            })
    }

    /// Имя как C-строка для NDK.
    pub fn as_c_str(&self) -> &CStr {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тест")]

    use super::*;

    #[test]
    fn a_real_package_name_is_kept_byte_for_byte() {
        let name = PackageName::new("dev.fasticamera.debug".to_owned()).expect("имя");
        assert_eq!(name.as_c_str().to_str().unwrap(), "dev.fasticamera.debug");
    }

    #[test]
    fn an_empty_name_is_rejected() {
        assert_eq!(PackageName::new(String::new()), Err(InvalidPackageName::Empty));
    }

    #[test]
    fn an_interior_nul_is_rejected_with_its_position() {
        assert_eq!(
            PackageName::new("dev\0fasticamera".to_owned()),
            Err(InvalidPackageName::InteriorNul { position: 3 })
        );
    }
}
