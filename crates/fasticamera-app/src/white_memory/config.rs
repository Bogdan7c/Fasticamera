//! Конфиг памяти белой: включена ли она и как называются её файлы (секция `camera.white_memory`).
//!
//! Режим `none` нужен приёмке на телефоне: сценарии `device-smoke.sh` не должны ни читать белую владельца,
//! ни портить её своими прогонами (в smoke скрипт всегда передаёт `none`; `device-deploy.sh` режим не трогает).

use serde::Deserialize;

/// Где живёт память белой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteMemoryMode {
    /// Файл в папке приложения: белая переживает перезапуск процесса.
    #[default]
    File,
    /// Памяти нет: при старте ничего не помнится, записи не делаются, файл не читается и не создаётся.
    None,
}

/// Настройки памяти белой.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteMemoryConfig {
    pub mode: WhiteMemoryMode,
    /// Имя файла памяти в папке приложения (простое имя, без каталогов).
    pub file_name: String,
    /// Суффикс временного файла записи: пишем `<file_name><суффикс>`, потом переименовываем поверх основного.
    pub temp_file_suffix: String,
}

impl Default for WhiteMemoryConfig {
    fn default() -> Self {
        Self {
            mode: WhiteMemoryMode::File,
            file_name: "white.json".to_owned(),
            temp_file_suffix: ".tmp".to_owned(),
        }
    }
}

/// Конфиг памяти белой некорректен; в тексте всегда есть имя поля.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WhiteMemoryConfigError {
    /// Имя пусто, содержит разделитель пути или равно «.» / «..».
    #[error("camera.white_memory.file_name: недопустимое имя файла {value:?} (нужно простое имя без каталогов)")]
    InvalidFileName { value: String },
    /// Пустой суффикс: временный файл совпал бы с основным и запись перестала быть атомарной.
    #[error("camera.white_memory.temp_file_suffix: суффикс временного файла не должен быть пустым")]
    EmptyTempSuffix,
    /// Суффикс с разделителем пути увёл бы временный файл из папки приложения.
    #[error("camera.white_memory.temp_file_suffix: суффикс {value:?} не должен содержать разделителей пути")]
    TempSuffixHasSeparator { value: String },
}

impl WhiteMemoryConfig {
    /// Проверяет имена файлов. В режиме `none` имена не используются, но всё равно проверяются: опечатка в
    /// конфиге не должна дожидаться того дня, когда память включат.
    pub fn validate(&self) -> Result<(), WhiteMemoryConfigError> {
        let name = self.file_name.as_str();
        let is_simple_name = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        if !is_simple_name {
            return Err(WhiteMemoryConfigError::InvalidFileName {
                value: self.file_name.clone(),
            });
        }
        if self.temp_file_suffix.is_empty() {
            return Err(WhiteMemoryConfigError::EmptyTempSuffix);
        }
        if self.temp_file_suffix.contains(['/', '\\']) {
            return Err(WhiteMemoryConfigError::TempSuffixHasSeparator {
                value: self.temp_file_suffix.clone(),
            });
        }
        Ok(())
    }

    /// Имя временного файла записи.
    pub fn temp_file_name(&self) -> String {
        format!("{}{}", self.file_name, self.temp_file_suffix)
    }
}
