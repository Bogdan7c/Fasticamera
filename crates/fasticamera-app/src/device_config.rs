//! Кто мы по паспорту телефона: производитель и модель для тегов Exif и DNG (`Make`, `Model`).
//!
//! Значения присылает Kotlin при `initialize` (`Build.MANUFACTURER`, `Build.MODEL`) секцией `device` — в Rust
//! они не хардкодятся (решение 106: факты процесса приходят из `initialize`). Секция необязательна: на ПК и в
//! тестах её нет, и это явное состояние ([`DeviceIdentityError::Absent`]), а не строка-заглушка. Настоящая
//! служба снимков без паспорта не стартует (приложение остаётся на фейке с видимой причиной), потому что
//! записать честный Exif/DNG без модели камеры нельзя.

use serde::{Deserialize, Serialize};

/// Секция `device` конфига нативной части.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DeviceConfig {
    /// `Build.MANUFACTURER`.
    pub make: Option<String>,
    /// `Build.MODEL`.
    pub model: Option<String>,
}

/// Проверенный паспорт телефона: оба поля заданы и не пусты.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    make: String,
    model: String,
}

impl DeviceIdentity {
    /// Производитель (Exif `Make`).
    pub fn make(&self) -> &str {
        &self.make
    }

    /// Модель телефона (Exif `Model`).
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// Почему паспорта телефона нет.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceIdentityError {
    /// Секции `device` или одного из её полей нет (Kotlin не прислал).
    #[error("device.{field}: Kotlin не прислал значение")]
    Absent { field: &'static str },
    /// Поле есть, но пустое: Exif с пустой моделью бесполезен.
    #[error("device.{field}: пустая строка")]
    Blank { field: &'static str },
}

impl DeviceConfig {
    /// Проверка при `initialize`: заданное поле не может быть пустым (отсутствие допустимо — его судит
    /// [`DeviceConfig::identity`] в момент, когда паспорт нужен).
    pub fn validate(&self) -> Result<(), DeviceIdentityError> {
        for (field, value) in [("make", &self.make), ("model", &self.model)] {
            if value.as_deref().is_some_and(|text| text.trim().is_empty()) {
                return Err(DeviceIdentityError::Blank { field });
            }
        }
        Ok(())
    }

    /// Паспорт телефона или причина, почему его нет.
    pub fn identity(&self) -> Result<DeviceIdentity, DeviceIdentityError> {
        self.validate()?;
        let make = self
            .make
            .as_deref()
            .ok_or(DeviceIdentityError::Absent { field: "make" })?;
        let model = self
            .model
            .as_deref()
            .ok_or(DeviceIdentityError::Absent { field: "model" })?;
        Ok(DeviceIdentity {
            make: make.trim().to_owned(),
            model: model.trim().to_owned(),
        })
    }
}

#[cfg(test)]
#[path = "device_config_tests.rs"]
mod tests;
