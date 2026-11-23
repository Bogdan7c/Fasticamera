//! Библиотека пресетов в памяти: встроенные + свои, активный пресет, правки поверх.
//!
//! Чистая модель, без ввода-вывода. Владеет инвариантами:
//! - имена уникальны без учёта регистра и не совпадают с именами встроенных;
//! - идентификаторы своих пресетов выдаются монотонно и не переиспользуются;
//! - активный пресет всегда существует;
//! - правки (рецепт: основа + ручки) лежат только поверх активного пресета и всегда ОТЛИЧАЮТСЯ от его рецепта;
//! - операция с ошибкой не меняет состояние.

use look_contract::{
    ActiveLook, BandAxis, BuiltinPreset, ColorWheel, EditState, GlobalKnob, HueBandId, LookControl, LookIntent,
    LookKnob, LookRecipe, LookSettings, PresetEntry, PresetError, PresetId, PresetKind, PresetName, StorageFailure,
    ToneBaseModel, UserPresetId, WheelPush, WheelZone,
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
                recipe: edited.clone(),
                preset_recipe: self.active_preset_recipe(),
                edits: EditState::EditedOnTop,
            },
            None => ActiveLook {
                preset: self.state.active,
                recipe: self.active_preset_recipe(),
                preset_recipe: self.active_preset_recipe(),
                edits: EditState::Clean,
            },
        }
    }

    /// Список для экрана: сначала встроенные, затем свои в порядке создания.
    pub fn presets(&self) -> Vec<PresetEntry> {
        let builtin = [BuiltinPreset::Honest, BuiltinPreset::AsEye]
            .into_iter()
            .map(|preset| PresetEntry {
                id: PresetId::Builtin(preset),
                name: self.catalog.name_of(preset).clone(),
                kind: PresetKind::Builtin,
            });
        let user = self.state.user_presets.iter().map(|preset| PresetEntry {
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
            LookIntent::SetKnob { knob, value } => self.set_knob(knob, value),
            LookIntent::SetBandAxis { band, axis, value } => self.set_band_axis(band, axis, value),
            LookIntent::SetWheelPush { zone, push } => self.set_wheel_push(zone, push),
            LookIntent::SetWheelRing { zone, ring } => self.set_wheel_ring(zone, ring),
            LookIntent::SetToneBase(base) => self.set_tone_base(base),
            LookIntent::ResetControl(control) => self.reset_control(control),
            LookIntent::SelectPreset(preset) => self.select_preset(preset),
            LookIntent::SaveAsNewPreset(name) => self.save_as_new_preset(name),
            LookIntent::RenamePreset { preset, name } => self.rename_preset(PresetId::User(preset), name),
            LookIntent::OverwritePreset(preset) => self.overwrite_preset(PresetId::User(preset)),
            LookIntent::DeletePreset(preset) => self.delete_preset(PresetId::User(preset)),
            LookIntent::DiscardEdits => self.discard_edits(),
        }
    }

    /// «Ручки = эти значения»: правки поверх активного пресета, основа остаётся текущей.
    /// Совпало с рецептом пресета → `Clean`.
    pub fn set_look(&mut self, settings: LookSettings) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let base = library.active_look().recipe.base;
            library.set_edited_recipe(LookRecipe { base, settings });
            Ok(())
        })
    }

    /// «Эта ручка = это значение» поверх ТЕКУЩЕГО вида (с правками): остальные ручки, полосы и основа не трогаются.
    /// Совпало с рецептом пресета → `Clean`. Два подряд намерения на разные ручки не теряют друг друга, потому что
    /// каждое применяется к актуальному рецепту библиотеки, а не к устаревшим фактам экрана.
    pub fn set_knob(&mut self, knob: GlobalKnob, value: LookKnob) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let mut recipe = library.active_look().recipe;
            recipe.settings = recipe.settings.with_knob(knob, value);
            library.set_edited_recipe(recipe);
            Ok(())
        })
    }

    /// «Эта ось этой полосы = это значение» поверх текущего вида; другие оси и полосы целы. Полосы нет в каталоге →
    /// `UnknownControl` (состояние не меняется).
    pub fn set_band_axis(
        &mut self,
        band: HueBandId,
        axis: BandAxis,
        value: LookKnob,
    ) -> Result<LibraryChange, PresetError> {
        if !self.catalog.knows_band(&band) {
            return Err(PresetError::UnknownControl(LookControl::Band { band, axis }));
        }
        self.transaction(|library| {
            let mut recipe = library.active_look().recipe;
            let adjustment = recipe.settings.bands.for_band(&band).with_axis(axis, value);
            recipe.settings.bands = recipe.settings.bands.with_adjustment(adjustment);
            library.set_edited_recipe(recipe);
            Ok(())
        })
    }

    /// «Толчок этого колеса = эта точка» поверх текущего вида: кольцо колеса, другие колёса, ручки, полосы и
    /// основа не трогаются. Исходы как у `set_knob`: точка та же → `Unchanged` (файл писать не надо); совпало
    /// с рецептом пресета → `Clean`.
    pub fn set_wheel_push(&mut self, zone: WheelZone, push: WheelPush) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let mut recipe = library.active_look().recipe;
            let wheel = ColorWheel {
                push,
                ..recipe.settings.wheels.wheel(zone)
            };
            recipe.settings.wheels = recipe.settings.wheels.with_wheel(zone, wheel);
            library.set_edited_recipe(recipe);
            Ok(())
        })
    }

    /// «Кольцо яркости этого колеса = это значение» поверх текущего вида: толчок колеса и всё остальное целы.
    /// Исходы те же, что у [`PresetLibrary::set_wheel_push`].
    pub fn set_wheel_ring(&mut self, zone: WheelZone, ring: LookKnob) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let mut recipe = library.active_look().recipe;
            let wheel = ColorWheel {
                ring,
                ..recipe.settings.wheels.wheel(zone)
            };
            recipe.settings.wheels = recipe.settings.wheels.with_wheel(zone, wheel);
            library.set_edited_recipe(recipe);
            Ok(())
        })
    }

    /// Сменить основу тона поверх активного пресета (решение 195): ручки остаются текущими.
    /// Вернули основу пресета и ручки тоже его → `Clean`.
    pub fn set_tone_base(&mut self, base: ToneBaseModel) -> Result<LibraryChange, PresetError> {
        self.transaction(|library| {
            let settings = library.active_look().recipe.settings;
            library.set_edited_recipe(LookRecipe { base, settings });
            Ok(())
        })
    }

    /// Вернуть один орган управления к значению активного ПРАВЛЕННОГО пресета, а не к нулю (решение 196).
    ///
    /// Различимые исходы: полосы нет в каталоге → `UnknownControl`; орган уже как у пресета →
    /// `Unchanged` (файл писать не надо); иначе сброшен только он, остальные правки целы, а если он был
    /// последним отличием — правок больше нет (`Clean`).
    pub fn reset_control(&mut self, control: LookControl) -> Result<LibraryChange, PresetError> {
        if let LookControl::Band { band, .. } = &control
            && !self.catalog.knows_band(band)
        {
            return Err(PresetError::UnknownControl(control));
        }
        self.transaction(|library| {
            let active = library.active_look();
            let reset = active.recipe.with_control_from(&control, &active.preset_recipe);
            library.set_edited_recipe(reset);
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
            let recipe = library.active_look().recipe;
            library.state.user_presets.push(UserPreset { id, name, recipe });
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
            let current_recipe = library.active_look().recipe;
            if let Some(target) = library.user_preset_mut(id) {
                target.recipe = current_recipe;
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
        // «Видимый вид» — итоговый рецепт (основа + ручки): именно он уходит рендеру. Смена активного
        // пресета или признака правок при тех же значениях картинку не меняет.
        let look_changed = candidate.active_look().recipe != self.active_look().recipe;
        let stored_changed = candidate.state != self.state;
        *self = candidate;
        Ok(match (look_changed, stored_changed) {
            (true, _) => LibraryChange::VisibleLookAndStoredState,
            (false, true) => LibraryChange::StoredStateOnly,
            (false, false) => LibraryChange::Unchanged,
        })
    }

    /// Записывает итоговый рецепт как правки поверх активного пресета; равный рецепту пресета → правок нет.
    fn set_edited_recipe(&mut self, recipe: LookRecipe) {
        self.state.edits = if recipe == self.active_preset_recipe() {
            None
        } else {
            Some(recipe)
        };
    }

    /// Рецепт активного пресета без правок.
    fn active_preset_recipe(&self) -> LookRecipe {
        match self.state.active {
            PresetId::Builtin(builtin) => self.catalog.recipe_of(builtin),
            PresetId::User(id) => self
                .state
                .user_presets
                .iter()
                .find(|preset| preset.id == id)
                .map(|preset| preset.recipe.clone())
                // Инвариант «активный существует» держит валидация и delete_preset.
                .unwrap_or_else(|| self.catalog.recipe_of(self.catalog.startup_preset())),
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
