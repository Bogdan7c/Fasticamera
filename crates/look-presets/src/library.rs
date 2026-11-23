//! Библиотека пресетов в памяти: встроенные + свои, активный пресет, правки поверх.
//!
//! Чистая модель, без ввода-вывода. Владеет инвариантами:
//! - имена уникальны без учёта регистра и не совпадают с именами встроенных;
//! - идентификаторы своих пресетов выдаются монотонно и не переиспользуются;
//! - активный пресет всегда существует;
//! - правки лежат только поверх активного пресета и всегда ОТЛИЧАЮТСЯ от его значений;
//! - операция с ошибкой не меняет состояние.

use look_contract::{
    ActiveLook, BuiltinPreset, EditState, LookIntent, LookSettings, PresetError, PresetId, PresetName, StorageFailure,
    UserPresetId,
};

use crate::config::BuiltinCatalog;
use crate::snapshot::{LibrarySnapshot, SnapshotError, UserPreset};

/// Что изменилось после операции: исполнителю важно знать, слать ли вид рендеру и писать ли файл.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryChange {
    /// Ничего: ни вид, ни сохраняемое состояние.
    Unchanged,
    /// Видимый вид тот же, но сохраняемое состояние изменилось (переименование, перезапись
    /// пресета текущим видом, удаление неактивного): рендеру слать нечего, файл писать надо.
    StoredStateOnly,
    /// Изменились итоговые значения ручек (а значит и сохраняемое состояние): слать рендеру и
    /// писать файл.
    VisibleLookAndStoredState,
}

impl LibraryChange {
    /// Нужно ли отправить новый вид рендеру.
    pub fn look_changed(self) -> bool {
        self == Self::VisibleLookAndStoredState
    }

    /// Нужно ли записать файл.
    pub fn needs_save(self) -> bool {
        self != Self::Unchanged
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
pub struct PresetSummary {
    /// Идентификатор.
    pub id: PresetId,
    /// Отображаемое имя.
    pub name: PresetName,
    /// Встроенный или свой.
    pub kind: PresetKind,
}

impl PresetSummary {
    /// Встроенный ли пресет.
    pub fn is_builtin(&self) -> bool {
        self.kind == PresetKind::Builtin
    }
}

/// Библиотека пресетов. Все изменения — через методы-намерения.
#[derive(Debug, Clone)]
pub struct PresetLibrary {
    catalog: BuiltinCatalog,
    state: LibrarySnapshot,
}

impl PresetLibrary {
    /// Чистая библиотека без своих пресетов: активен стартовый встроенный (решение 98).
    pub fn new(catalog: BuiltinCatalog) -> Self {
        let state = LibrarySnapshot {
            next_id: 1,
            active: PresetId::Builtin(catalog.startup_preset()),
            edits: None,
            user_presets: Vec::new(),
        };
        Self { catalog, state }
    }

    /// Библиотека из сохранённого состояния; нарушение инвариантов — ошибка (файл испорчен).
    pub fn from_snapshot(catalog: BuiltinCatalog, snapshot: LibrarySnapshot) -> Result<Self, SnapshotError> {
        snapshot.validate(&catalog)?;
        Ok(Self {
            catalog,
            state: snapshot,
        })
    }

    /// Сохраняемое состояние (копия) для записи в файл.
    pub fn snapshot(&self) -> LibrarySnapshot {
        self.state.clone()
    }

    /// Текущий вид: активный пресет, итоговые значения, есть ли правки поверх.
    pub fn active_look(&self) -> ActiveLook {
        match &self.state.edits {
            Some(edited) => ActiveLook {
                preset: self.state.active,
                settings: edited.clone(),
                edits: EditState::EditedOnTop,
            },
            None => ActiveLook {
                preset: self.state.active,
                settings: self.active_preset_settings(),
                edits: EditState::Clean,
            },
        }
    }

    /// Список для экрана: сначала встроенные, затем свои в порядке создания.
    pub fn presets(&self) -> Vec<PresetSummary> {
        let builtin = [BuiltinPreset::Honest, BuiltinPreset::AsEye]
            .into_iter()
            .map(|preset| PresetSummary {
                id: PresetId::Builtin(preset),
                name: self.catalog.name_of(preset).clone(),
                kind: PresetKind::Builtin,
            });
        let user = self.state.user_presets.iter().map(|preset| PresetSummary {
            id: PresetId::User(preset.id),
            name: preset.name.clone(),
            kind: PresetKind::User,
        });
        builtin.chain(user).collect()
    }

    /// Единая точка применения намерения экрана.
    pub fn apply(&mut self, intent: LookIntent) -> Result<LibraryChange, PresetError> {
        match intent {
            LookIntent::SetLook(settings) => self.set_look(settings),
            LookIntent::SelectPreset(preset) => self.select_preset(preset),
            LookIntent::SaveAsNewPreset(name) => self.save_as_new_preset(name),
            LookIntent::RenamePreset { preset, name } => self.rename_preset(PresetId::User(preset), name),
            LookIntent::OverwritePreset(preset) => self.overwrite_preset(PresetId::User(preset)),
            LookIntent::DeletePreset(preset) => self.delete_preset(PresetId::User(preset)),
            LookIntent::DiscardEdits => self.discard_edits(),
        }
    }

    /// «Вид = эти значения»: правки поверх активного. Совпало со значениями пресета → `Clean`.
    pub fn set_look(&mut self, settings: LookSettings) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            library.state.edits = if settings == library.active_preset_settings() {
                None
            } else {
                Some(settings)
            };
            Ok(())
        })
    }

    /// Выбрать пресет: активным становится он, правки сбрасываются.
    pub fn select_preset(&mut self, preset: PresetId) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            library.require_exists(preset)?;
            library.state.active = preset;
            library.state.edits = None;
            Ok(())
        })
    }

    /// Сохранить текущий вид как новый свой пресет; он становится активным, `Clean`.
    pub fn save_as_new_preset(&mut self, name: PresetName) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            library.require_name_free(&name, None)?;
            let id = UserPresetId(library.state.next_id);
            let next_id = library
                .state
                .next_id
                .checked_add(1)
                .ok_or(PresetError::Storage(StorageFailure::IdSpaceExhausted))?;
            let settings = library.active_look().settings;
            library.state.user_presets.push(UserPreset { id, name, settings });
            library.state.next_id = next_id;
            library.state.active = PresetId::User(id);
            library.state.edits = None;
            Ok(())
        })
    }

    /// Переименовать свой пресет. Встроенный — `BuiltinIsReadOnly`.
    pub fn rename_preset(&mut self, preset: PresetId, name: PresetName) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let id = library.require_user_preset(preset)?;
            // Своё прежнее имя не считается занятым: можно сменить только регистр.
            library.require_name_free(&name, Some(id))?;
            if let Some(target) = library.user_preset_mut(id) {
                target.name = name;
            }
            Ok(())
        })
    }

    /// Записать текущий вид в свой пресет; пресет становится активным, `Clean`.
    pub fn overwrite_preset(&mut self, preset: PresetId) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let id = library.require_user_preset(preset)?;
            let current_look = library.active_look().settings;
            if let Some(target) = library.user_preset_mut(id) {
                target.settings = current_look;
            }
            library.state.active = PresetId::User(id);
            library.state.edits = None;
            Ok(())
        })
    }

    /// Удалить свой пресет. Удаление активного возвращает стартовый встроенный («Как глаз»
    /// по умолчанию) и сбрасывает правки, потому что они лежали поверх удалённого.
    pub fn delete_preset(&mut self, preset: PresetId) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let id = library.require_user_preset(preset)?;
            library.state.user_presets.retain(|existing| existing.id != id);
            if library.state.active == PresetId::User(id) {
                library.state.active = PresetId::Builtin(library.catalog.startup_preset());
                library.state.edits = None;
            }
            Ok(())
        })
    }

    /// Отбросить правки поверх: вернуть значения активного пресета.
    pub fn discard_edits(&mut self) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            library.state.edits = None;
            Ok(())
        })
    }

    /// Выполняет изменение на копии состояния: при ошибке оригинал не тронут, при успехе
    /// по сравнению «до/после» вычисляется [`LibraryChange`]. Так операции не могут
    /// забыть сообщить об изменении или оставить половину изменений после ошибки.
    fn transaction(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<(), PresetError>,
    ) -> Result<LibraryChange, PresetError> {
        let mut candidate = self.clone();
        change(&mut candidate)?;
        // «Видимый вид» — итоговые значения ручек: именно они уходят рендеру. Смена активного
        // пресета или признака правок при тех же значениях картинку не меняет.
        let look_changed = candidate.active_look().settings != self.active_look().settings;
        let stored_changed = candidate.state != self.state;
        *self = candidate;
        Ok(match (look_changed, stored_changed) {
            (true, _) => LibraryChange::VisibleLookAndStoredState,
            (false, true) => LibraryChange::StoredStateOnly,
            (false, false) => LibraryChange::Unchanged,
        })
    }

    /// Значения активного пресета без правок.
    fn active_preset_settings(&self) -> LookSettings {
        match self.state.active {
            PresetId::Builtin(builtin) => self.catalog.settings_of(builtin),
            PresetId::User(id) => self
                .state
                .user_presets
                .iter()
                .find(|preset| preset.id == id)
                .map(|preset| preset.settings.clone())
                // Инвариант «активный существует» держит валидация и delete_preset.
                .unwrap_or_default(),
        }
    }

    fn require_exists(&self, preset: PresetId) -> Result<(), PresetError> {
        match preset {
            PresetId::Builtin(_) => Ok(()),
            PresetId::User(id) if self.state.user_presets.iter().any(|p| p.id == id) => Ok(()),
            PresetId::User(_) => Err(PresetError::NotFound(preset)),
        }
    }

    /// Свой пресет по id; встроенный — `BuiltinIsReadOnly`, неизвестный — `NotFound`.
    fn require_user_preset(&self, preset: PresetId) -> Result<UserPresetId, PresetError> {
        match preset {
            PresetId::Builtin(builtin) => Err(PresetError::BuiltinIsReadOnly(builtin)),
            PresetId::User(id) => self.require_exists(preset).map(|()| id),
        }
    }

    fn user_preset_mut(&mut self, id: UserPresetId) -> Option<&mut UserPreset> {
        self.state.user_presets.iter_mut().find(|preset| preset.id == id)
    }

    /// Имя свободно, если его не занял ни встроенный, ни другой свой пресет.
    fn require_name_free(&self, name: &PresetName, except: Option<UserPresetId>) -> Result<(), PresetError> {
        let taken_by_user = self
            .state
            .user_presets
            .iter()
            .filter(|preset| Some(preset.id) != except)
            .any(|preset| name.same_name_as(preset.name.as_str()));
        if taken_by_user || self.catalog.is_builtin_name(name) {
            return Err(PresetError::NameTaken);
        }
        Ok(())
    }
}
