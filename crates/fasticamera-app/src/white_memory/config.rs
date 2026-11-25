//! Конфиг памяти белой: включена ли она и как называются её файлы (секция `camera.white_memory`).
//!
//! Режим `none` нужен приёмке на телефоне: сценарии `device-smoke.sh` не должны ни читать белую владельца,
//! ни портить её своими прогонами (в smoke скрипт всегда передаёт `none`; `device-deploy.sh` режим не трогает).

use std::time::Duration;

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
    /// Не чаще одной записи на диск за столько мс. Шкала белой под пальцем шлёт правки десятки раз в секунду;
    /// последнее значение всё равно ложится не позже этого срока, а при уходе в фон и закрытии — сразу.
    pub min_write_interval_ms: u64,
}

impl Default for WhiteMemoryConfig {
    fn default() -> Self {
        Self {
            mode: WhiteMemoryMode::File,
            file_name: "white.json".to_owned(),
            temp_file_suffix: ".tmp".to_owned(),
            min_write_interval_ms: 250,
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
    /// Ноль выключил бы ограничение частоты записи, слишком большой срок оставил бы белую на диске далеко позади.
    #[error(
        "camera.white_memory.min_write_interval_ms: {value} мс вне допустимого (от 1 до {max} мс)",
        max = WhiteMemoryConfig::MAX_WRITE_INTERVAL_MS
    )]
    InvalidWriteInterval { value: u64 },
}

impl WhiteMemoryConfig {
    /// Верхний предел интервала записи, мс: дольше значение не должно ждать в ящике (процесс могут убить).
    pub const MAX_WRITE_INTERVAL_MS: u64 = 10_000;

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
        if !(1..=Self::MAX_WRITE_INTERVAL_MS).contains(&self.min_write_interval_ms) {
            return Err(WhiteMemoryConfigError::InvalidWriteInterval {
                value: self.min_write_interval_ms,
            });
        }
        Ok(())
    }

    /// Минимальный срок между записями на диск.
    pub fn min_write_interval(&self) -> Duration {
        Duration::from_millis(self.min_write_interval_ms)
    }

    /// Имя временного файла записи.
    pub fn temp_file_name(&self) -> String {
        format!("{}{}", self.file_name, self.temp_file_suffix)
    }
}
