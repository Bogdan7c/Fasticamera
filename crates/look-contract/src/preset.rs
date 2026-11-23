//! Идентификаторы и имена пресетов.

use serde::{Deserialize, Serialize};

/// Максимальная длина имени пресета в символах.
pub const PRESET_NAME_MAX_CHARS: usize = 40;

/// Встроенные пресеты (решение 98). Только читаются: изменить или удалить нельзя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinPreset {
    /// «Честный»: слой вида выключен, все ручки в нуле.
    Honest,
    /// «Как глаз»: пресет по умолчанию, значения задаёт конфиг хранилища.
    AsEye,
}

/// Идентификатор пользовательского пресета. Выдаётся хранилищем монотонно и не переиспользуется.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserPresetId(pub u64);

/// Любой пресет: встроенный или свой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresetId {
    /// Встроенный.
    Builtin(BuiltinPreset),
    /// Пользовательский.
    User(UserPresetId),
}

/// Ошибка имени пресета.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PresetNameError {
    /// После обрезки пробелов имя пусто.
    #[error("имя пресета пусто")]
    Empty,
    /// Слишком длинное имя.
    #[error("имя пресета длиннее {max} символов: {actual}")]
    TooLong {
        /// Допустимый максимум.
        max: usize,
        /// Фактическая длина после обрезки.
        actual: usize,
    },
    /// Управляющий символ (перевод строки, табуляция, NUL…) внутри имени.
    #[error("имя пресета содержит управляющий символ {found:?}")]
    ControlCharacter {
        /// Первый найденный управляющий символ.
        found: char,
    },
}

/// Проверенное имя пресета: обрезанное по краям, 1..=40 символов, без управляющих символов.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PresetName(String);

impl PresetName {
    /// Проверяет и строит имя (лишние пробелы по краям срезаются молча: это ввод человека).
    pub fn new(raw: impl AsRef<str>) -> Result<Self, PresetNameError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(PresetNameError::Empty);
        }
        if let Some(found) = trimmed.chars().find(|c| c.is_control()) {
            return Err(PresetNameError::ControlCharacter { found });
        }
        let actual = trimmed.chars().count();
        if actual > PRESET_NAME_MAX_CHARS {
            return Err(PresetNameError::TooLong {
                max: PRESET_NAME_MAX_CHARS,
                actual,
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Имя как строка.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Совпадают ли имена для проверки «занято»: без учёта регистра и пробелов по краям.
    pub fn same_name_as(&self, other: &str) -> bool {
        self.0.to_lowercase() == other.trim().to_lowercase()
    }
}

impl TryFrom<String> for PresetName {
    type Error = PresetNameError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(raw)
    }
}

impl From<PresetName> for String {
    fn from(name: PresetName) -> Self {
        name.0
    }
}

/// Встроенный или свой пресет (для списка на экране).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    /// Встроенный: не меняется и не удаляется.
    Builtin,
    /// Свой.
    User,
}

/// Строка списка пресетов для экрана.
#[derive(Debug, Clone, PartialEq)]
pub struct PresetEntry {
    /// Идентификатор.
    pub id: PresetId,
    /// Отображаемое имя.
    pub name: PresetName,
    /// Встроенный или свой.
    pub kind: PresetKind,
}

impl PresetEntry {
    /// Встроенный ли пресет.
    pub fn is_builtin(&self) -> bool {
        self.kind == PresetKind::Builtin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_trimmed() {
        assert_eq!(PresetName::new("  Sunset \n").unwrap().as_str(), "Sunset");
    }

    #[test]
    fn name_length_edges_count_chars_not_bytes() {
        assert!(PresetName::new("я".repeat(40)).is_ok());
        assert_eq!(
            PresetName::new("я".repeat(41)),
            Err(PresetNameError::TooLong { max: 40, actual: 41 })
        );
    }

    #[test]
    fn empty_and_whitespace_only_are_rejected() {
        assert_eq!(PresetName::new(""), Err(PresetNameError::Empty));
        assert_eq!(PresetName::new(" \t "), Err(PresetNameError::Empty));
    }

    #[test]
    fn control_characters_inside_are_rejected() {
        assert_eq!(
            PresetName::new("a\nb"),
            Err(PresetNameError::ControlCharacter { found: '\n' })
        );
        assert!(PresetName::new("a\u{0}b").is_err());
    }

    #[test]
    fn same_name_ignores_case_and_edge_spaces() {
        let name = PresetName::new("Natural").unwrap();
        assert!(name.same_name_as("nATURAL"));
        assert!(name.same_name_as("  natural "));
        assert!(!name.same_name_as("Natura"));
        // Юникод-регистр тоже сворачивается.
        assert!(PresetName::new("Закат").unwrap().same_name_as("ЗАКАТ"));
    }

    #[test]
    fn serde_rejects_empty_name_and_roundtrips_valid() {
        assert!(serde_json::from_str::<PresetName>("\"   \"").is_err());
        let name: PresetName = serde_json::from_str("\" Warm \"").unwrap();
        assert_eq!(name.as_str(), "Warm");
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"Warm\"");
    }

    #[test]
    fn preset_id_has_stable_json_shape() {
        let builtin = PresetId::Builtin(BuiltinPreset::AsEye);
        let user = PresetId::User(UserPresetId(7));
        assert_eq!(serde_json::to_string(&builtin).unwrap(), r#"{"builtin":"as_eye"}"#);
        assert_eq!(serde_json::to_string(&user).unwrap(), r#"{"user":7}"#);
        assert_eq!(serde_json::from_str::<PresetId>(r#"{"user":7}"#).unwrap(), user);
    }
}
