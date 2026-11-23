//! Сохраняемое состояние библиотеки пресетов — ровно то, что лежит в файле.

use look_contract::{LookRecipe, LookSettings, PresetId, PresetName, ToneBaseModel, UserPresetId};
use serde::{Deserialize, Serialize};

use crate::config::BuiltinCatalog;

/// Свой пресет пользователя: имя и полный рецепт (основа + ручки, решение 195).
///
/// В файле схемы 3 основа и ручки лежат рядом с именем (`id`, `name`, `base`, `settings`), а не во вложенном
/// объекте: так шаг миграции 2 → 3 только ДОБАВЛЯЕТ поле `base` и не двигает остальное.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "UserPresetWire", into = "UserPresetWire")]
pub struct UserPreset {
    /// Идентификатор (не переиспользуется после удаления).
    pub id: UserPresetId,
    /// Имя.
    pub name: PresetName,
    /// Рецепт: основа тона и значения ручек.
    pub recipe: LookRecipe,
}

/// Форма свёрнутого пресета в файле (см. [`UserPreset`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserPresetWire {
    id: UserPresetId,
    name: PresetName,
    base: ToneBaseModel,
    settings: LookSettings,
}

impl From<UserPresetWire> for UserPreset {
    fn from(wire: UserPresetWire) -> Self {
        Self {
            id: wire.id,
            name: wire.name,
            recipe: LookRecipe {
                base: wire.base,
                settings: wire.settings,
            },
        }
    }
}

impl From<UserPreset> for UserPresetWire {
    fn from(preset: UserPreset) -> Self {
        Self {
            id: preset.id,
            name: preset.name,
            base: preset.recipe.base,
            settings: preset.recipe.settings,
        }
    }
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
    pub edits: Option<LookRecipe>,
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
