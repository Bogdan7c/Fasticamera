//! Владелец библиотеки пресетов вида в приложении (решения 99, 178, 179).
//!
//! Что здесь живёт:
//! * [`LookService`] — библиотека пресетов (`look-presets`) + канал записи файла. Применяет намерение экрана и
//!   говорит, что из этого следует: нужно ли слать вид рендеру и просить ли запись. Рендеру сам НИЧЕГО не шлёт
//!   (это делает исполнитель эффектов, `look_effects`), диска не ждёт (запись — отдельный поток).
//! * [`LookHandle`] — общая ручка на сервис. Библиотека обязана пережить экран: экран камеры ставится и
//!   переставляется, а исполнитель эффектов умирает вместе с ним (`EffectExecutor` — «конец жизни экрана»).
//!   Поэтому сервис создаётся один раз при старте процесса, а исполнитель каждого экрана берёт ту же ручку.
//!   Замок берёт только исполнитель на потоке рендера, споров за него нет; он нужен ради `Send + Sync`.
//! * [`LookPersistence`] — узкий шов к потоку записи: в продукте это `PresetPersistence`, в тестах подставной.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use look_contract::{ActiveLook, LookIntent, LookProblemKind, LookRecipe, LookSettings, PresetEntry, PresetError};
use look_presets::{
    CatalogEnvironment, LibraryChange, LibrarySnapshot, LoadOutcome, PresetLibrary, PresetPersistence,
    PresetStoreConfig, SaveOutcome, SaveRequestOutcome,
};

/// Шов к записи файла: просьба записать снимок и опрос итогов. Не блокирует на диске.
pub trait LookPersistence: Send {
    /// Просит записать снимок библиотеки.
    fn request_save(&self, snapshot: LibrarySnapshot) -> SaveRequestOutcome;
    /// Забирает накопленные итоги записей (старые первыми).
    fn drain_outcomes(&self) -> Vec<SaveOutcome>;
}

impl LookPersistence for PresetPersistence {
    fn request_save(&self, snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        PresetPersistence::request_save(self, snapshot)
    }

    fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        PresetPersistence::drain_outcomes(self)
    }
}

/// Записи нет вообще (сборка без папки приложения: сценарии на ПК, тесты). Каждая просьба честно отвечает
/// «приёмник закрыт», итогов не бывает: вид живёт только в памяти до конца процесса.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoLookPersistence;

impl LookPersistence for NoLookPersistence {
    fn request_save(&self, _snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        SaveRequestOutcome::Closed
    }

    fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        Vec::new()
    }
}

/// Как прошёл запуск хранилища пресетов (факт для диагностики; ошибки при этом не молчаливые: см. `tracing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LookStartup {
    /// Файл не открывался: библиотека только в памяти (сборка без папки приложения).
    #[default]
    InMemoryOnly,
    /// Файла ещё нет: чистая библиотека со встроенными пресетами.
    NoFileYet,
    /// Файл прочитан как есть.
    Loaded,
    /// Файл прочитан после миграции со старой схемы.
    Migrated {
        /// Версия схемы в файле.
        from_version: u32,
    },
    /// Файл был испорчен или новее приложения и убран в карантин (`…corrupt-<время>`); работаем с чистой библиотекой.
    Quarantined,
    /// Файл испорчен, а убрать его в карантин не удалось: файл не тронут, сохранения отключены.
    QuarantineFailed,
    /// Файл есть, но прочитать его не удалось: файл не тронут, сохранения отключены.
    Unreadable,
}

impl LookStartup {
    /// Факт запуска по исходу загрузки файла.
    pub fn of_outcome(outcome: &LoadOutcome) -> Self {
        match outcome {
            LoadOutcome::Missing => Self::NoFileYet,
            LoadOutcome::Loaded { migrated_from: None } => Self::Loaded,
            LoadOutcome::Loaded {
                migrated_from: Some(from_version),
            } => Self::Migrated {
                from_version: *from_version,
            },
            LoadOutcome::Quarantined { .. } => Self::Quarantined,
            LoadOutcome::QuarantineFailed { .. } => Self::QuarantineFailed,
            LoadOutcome::Unreadable { .. } => Self::Unreadable,
        }
    }
}

impl LookStartup {
    /// Проблема, о которой владельцу надо сказать плашкой на экране (решение 287): файл сброшен в карантин или
    /// сохранения отключены. Нормальные исходы (нет файла, прочитан, мигрирован, только память) — `None`: ни
    /// «файла ещё нет», ни миграция плашки не заслуживают.
    pub fn problem(self) -> Option<LookProblemKind> {
        match self {
            Self::Quarantined => Some(LookProblemKind::PresetsReset),
            Self::QuarantineFailed | Self::Unreadable => Some(LookProblemKind::SavesRefused),
            Self::InMemoryOnly | Self::NoFileYet | Self::Loaded | Self::Migrated { .. } => None,
        }
    }
}

/// Что следует из применённого намерения. Исполнитель эффектов по этому решает, что делать дальше.
#[derive(Debug, Clone, PartialEq)]
pub struct LookApplied {
    /// Что изменилось в библиотеке.
    pub change: LibraryChange,
    /// Новый вид для рендера: `Some`, только если итоговые значения ручек изменились.
    pub look_for_render: Option<Arc<LookSettings>>,
    /// Что стало с просьбой записать файл: `None`, если записывать было нечего.
    pub save_request: Option<SaveRequestOutcome>,
}

/// Всё, что экрану нужно знать о библиотеке за один раз: текущий вид и список пресетов. Снимается под ОДНИМ замком
/// ручки, поэтому список и активный пресет не разойдутся (между двумя чтениями мог бы встрять чужой поток).
#[derive(Debug, Clone, PartialEq)]
pub struct LookViewState {
    /// Активный пресет, итоговый рецепт и рецепт пресета без правок.
    pub active: ActiveLook,
    /// Список пресетов: сначала встроенные, затем свои.
    pub presets: Vec<PresetEntry>,
}

/// Библиотека пресетов вместе с каналом записи.
pub struct LookService {
    library: PresetLibrary,
    persistence: Box<dyn LookPersistence>,
    startup: LookStartup,
    /// Проблема запуска, которую экран ещё не ПОКАЗАЛ плашкой. Сервис живёт дольше любого экрана и держит проблему,
    /// пока экран не подтвердит показ (`acknowledge_startup_problem`): пересозданный до входа в раздел экран получит
    /// её снова, а показанную — уже нет (плашка один раз за жизнь процесса).
    undelivered_startup_problem: Option<LookProblemKind>,
    /// Номер версии библиотеки: растёт на каждое намерение, что-либо изменившее (активный вид, список пресетов,
    /// имена, правки). Владеет им сервис, потому что только он применяет намерения; наблюдатели фактов по нему дёшево
    /// узнают «ничего не менялось» без пересборки снимка (см. `look_facts`).
    revision: u64,
}

impl LookService {
    /// Сервис поверх готовой библиотеки (её открыл `PresetFileStore::open` или создали чистой).
    pub fn new(library: PresetLibrary, persistence: Box<dyn LookPersistence>, startup: LookStartup) -> Self {
        Self {
            library,
            persistence,
            startup,
            undelivered_startup_problem: startup.problem(),
            revision: 0,
        }
    }

    /// Номер версии библиотеки: меняется ровно тогда, когда изменилось сохраняемое состояние (а значит, и всё, что
    /// видно экрану). Отказ библиотеки и «ничего не изменилось» номер не двигают.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Сервис только в памяти: встроенные пресеты из конфига, файла нет (ПК, тесты, сборка без папки приложения).
    pub fn in_memory(
        config: &PresetStoreConfig,
        environment: &CatalogEnvironment,
    ) -> Result<Self, look_presets::ConfigError> {
        let library = PresetLibrary::new(config.builtin_catalog(environment)?);
        Ok(Self::new(
            library,
            Box::new(NoLookPersistence),
            LookStartup::InMemoryOnly,
        ))
    }

    /// Применяет намерение. Ошибка библиотеки отдаётся как есть (имя занято, не найден…): состояние при этом
    /// не меняется. Просьба записать файл уходит здесь же, но диска не ждёт.
    pub fn apply(&mut self, intent: LookIntent) -> Result<LookApplied, PresetError> {
        let change = self.library.apply(intent)?;
        if change.needs_save() {
            // `needs_save` ⇔ изменилось сохраняемое состояние, из которого собираются все факты экрана.
            self.revision += 1;
        }
        let look_for_render = change
            .look_changed()
            .then(|| Arc::new(self.library.active_look().recipe.settings));
        let save_request = change
            .needs_save()
            .then(|| self.persistence.request_save(self.library.snapshot()));
        Ok(LookApplied {
            change,
            look_for_render,
            save_request,
        })
    }

    /// Текущий вид: активный пресет, итоговые значения и признак правок поверх.
    pub fn active_look(&self) -> ActiveLook {
        self.library.active_look()
    }

    /// Текущий вид и список пресетов одним снимком (см. [`LookViewState`]).
    pub fn view_state(&self) -> LookViewState {
        LookViewState {
            active: self.library.active_look(),
            presets: self.library.presets(),
        }
    }

    /// Итоговые значения ручек сейчас — то, что уходит рендеру и в запрос снимка.
    pub fn active_settings(&self) -> LookSettings {
        self.library.active_look().recipe.settings
    }

    /// Итоговый рецепт сейчас: основа тона и значения ручек.
    pub fn active_recipe(&self) -> LookRecipe {
        self.library.active_look().recipe
    }

    /// Забирает итоги записей файла.
    pub fn drain_save_outcomes(&mut self) -> Vec<SaveOutcome> {
        self.persistence.drain_outcomes()
    }

    /// Как прошёл запуск хранилища.
    pub fn startup(&self) -> LookStartup {
        self.startup
    }

    /// Проблема запуска, которую экран ещё не показал; не снимается сама, пока нет подтверждения показа.
    pub fn pending_startup_problem(&self) -> Option<LookProblemKind> {
        self.undelivered_startup_problem.clone()
    }

    /// Экран показал плашку проблемы запуска: больше её не отдавать.
    pub fn acknowledge_startup_problem(&mut self) {
        self.undelivered_startup_problem = None;
    }
}

/// Общая ручка на [`LookService`] (см. доку модуля: библиотека переживает экран).
#[derive(Clone)]
pub struct LookHandle {
    service: Arc<Mutex<LookService>>,
}

impl LookHandle {
    /// Оборачивает сервис в общую ручку.
    pub fn new(service: LookService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
        }
    }

    /// Ручка на сервис только в памяти (см. [`LookService::in_memory`]).
    pub fn in_memory(
        config: &PresetStoreConfig,
        environment: &CatalogEnvironment,
    ) -> Result<Self, look_presets::ConfigError> {
        LookService::in_memory(config, environment).map(Self::new)
    }

    fn lock(&self) -> MutexGuard<'_, LookService> {
        // Паника в другом потоке не должна обездвиживать вид: состояние библиотеки остаётся целым
        // (операции библиотеки с ошибкой его не меняют).
        self.service.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// См. [`LookService::apply`].
    pub fn apply(&self, intent: LookIntent) -> Result<LookApplied, PresetError> {
        self.lock().apply(intent)
    }

    /// См. [`LookService::revision`].
    pub fn revision(&self) -> u64 {
        self.lock().revision()
    }

    /// См. [`LookService::active_look`].
    pub fn active_look(&self) -> ActiveLook {
        self.lock().active_look()
    }

    /// См. [`LookService::view_state`].
    pub fn view_state(&self) -> LookViewState {
        self.lock().view_state()
    }

    /// См. [`LookService::active_settings`].
    pub fn active_settings(&self) -> LookSettings {
        self.lock().active_settings()
    }

    /// См. [`LookService::active_recipe`].
    pub fn active_recipe(&self) -> LookRecipe {
        self.lock().active_recipe()
    }

    /// См. [`LookService::drain_save_outcomes`].
    pub fn drain_save_outcomes(&self) -> Vec<SaveOutcome> {
        self.lock().drain_save_outcomes()
    }

    /// См. [`LookService::startup`].
    pub fn startup(&self) -> LookStartup {
        self.lock().startup()
    }

    /// См. [`LookService::pending_startup_problem`].
    pub fn pending_startup_problem(&self) -> Option<LookProblemKind> {
        self.lock().pending_startup_problem()
    }

    /// См. [`LookService::acknowledge_startup_problem`].
    pub fn acknowledge_startup_problem(&self) {
        self.lock().acknowledge_startup_problem();
    }
}

#[cfg(test)]
#[path = "look_service_tests.rs"]
mod tests;
