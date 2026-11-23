//! Сохраняемое состояние библиотеки пресетов — ровно то, что лежит в файле.

use look_contract::{LookSettings, PresetId, PresetName, UserPresetId};
use serde::{Deserialize, Serialize};

use crate::config::BuiltinCatalog;

/// Свой пресет пользователя.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserPreset {
    /// Идентификатор (не переиспользуется после удаления).
    pub id: UserPresetId,
    /// Имя.
    pub name: PresetName,
    /// Значения ручек.
    pub settings: LookSettings,
}

/// Что именно сохраняется между запусками. Встроенные пресеты сюда НЕ входят: их значения
/// и имена приходят из конфига, поэтому подбор владельца в конфиге действует сразу.
#[derive(Debug, Clone, PartialEq)]
pub struct LibrarySnapshot {
    /// Следующий свободный идентификатор своего пресета (монотонный счётчик).
    pub next_id: u64,
    /// Активный пресет.
    pub active: PresetId,
    /// Правки поверх активного пресета; `None` — значения как у пресета.
    pub edits: Option<LookSettings>,
    /// Свои пресеты в порядке создания.
    pub user_presets: Vec<UserPreset>,
}

/// Содержимое снимка нарушает инварианты библиотеки (файл правили руками или он повреждён).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    /// Два своих пресета с одним идентификатором.
    #[error("повтор идентификатора пресета {0:?}")]
    DuplicateId(UserPresetId),
    /// Два своих пресета с одним именем (без учёта регистра).
    #[error("повтор имени пресета {0:?}")]
    DuplicateName(String),
    /// Имя своего пресета совпало с именем встроенного.
    #[error("имя пресета {0:?} занято встроенным")]
    NameClashesWithBuiltin(String),
    /// Активный пресет не существует.
    #[error("активный пресет {0:?} не найден")]
    ActivePresetMissing(PresetId),
    /// Счётчик идентификаторов не больше уже выданных: следующий пресет получил бы чужой id.
    #[error("next_id {next_id} не больше выданного идентификатора {max_used}")]
    NextIdNotAfterUsed {
        /// Значение счётчика из файла.
        next_id: u64,
        /// Наибольший выданный идентификатор.
        max_used: u64,
    },
}

impl LibrarySnapshot {
    /// Проверяет инварианты относительно каталога встроенных.
    pub fn validate(&self, catalog: &BuiltinCatalog) -> Result<(), SnapshotError> {
        for (index, preset) in self.user_presets.iter().enumerate() {
            let earlier = &self.user_presets[..index];
            if earlier.iter().any(|other| other.id == preset.id) {
                return Err(SnapshotError::DuplicateId(preset.id));
            }
            if earlier
                .iter()
                .any(|other| other.name.same_name_as(preset.name.as_str()))
            {
                return Err(SnapshotError::DuplicateName(preset.name.as_str().to_owned()));
            }
            if catalog.is_builtin_name(&preset.name) {
                return Err(SnapshotError::NameClashesWithBuiltin(preset.name.as_str().to_owned()));
            }
            if self.next_id <= preset.id.0 {
                return Err(SnapshotError::NextIdNotAfterUsed {
                    next_id: self.next_id,
                    max_used: preset.id.0,
                });
            }
        }
        if let PresetId::User(id) = self.active
            && !self.user_presets.iter().any(|preset| preset.id == id)
        {
            return Err(SnapshotError::ActivePresetMissing(self.active));
        }
        Ok(())
    }
}
