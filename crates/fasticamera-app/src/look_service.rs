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

use look_contract::{ActiveLook, LookIntent, LookSettings, PresetError};
use look_presets::{
    LibraryChange, LibrarySnapshot, LoadOutcome, PresetLibrary, PresetPersistence, PresetStoreConfig, SaveOutcome,
    SaveRequestOutcome,
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

/// Библиотека пресетов вместе с каналом записи.
pub struct LookService {
    library: PresetLibrary,
    persistence: Box<dyn LookPersistence>,
    startup: LookStartup,
}

impl LookService {
    /// Сервис поверх готовой библиотеки (её открыл `PresetFileStore::open` или создали чистой).
    pub fn new(library: PresetLibrary, persistence: Box<dyn LookPersistence>, startup: LookStartup) -> Self {
        Self {
            library,
            persistence,
            startup,
        }
    }

    /// Сервис только в памяти: встроенные пресеты из конфига, файла нет (ПК, тесты, сборка без папки приложения).
    pub fn in_memory(config: &PresetStoreConfig) -> Result<Self, look_presets::ConfigError> {
        let library = PresetLibrary::new(config.builtin_catalog()?);
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
        let look_for_render = change
            .look_changed()
            .then(|| Arc::new(self.library.active_look().settings));
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

    /// Итоговые значения ручек сейчас — то, что уходит рендеру и в запрос снимка.
    pub fn active_settings(&self) -> LookSettings {
        self.library.active_look().settings
    }

    /// Забирает итоги записей файла.
    pub fn drain_save_outcomes(&mut self) -> Vec<SaveOutcome> {
        self.persistence.drain_outcomes()
    }

    /// Как прошёл запуск хранилища.
    pub fn startup(&self) -> LookStartup {
        self.startup
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
    pub fn in_memory(config: &PresetStoreConfig) -> Result<Self, look_presets::ConfigError> {
        LookService::in_memory(config).map(Self::new)
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

    /// См. [`LookService::active_look`].
    pub fn active_look(&self) -> ActiveLook {
        self.lock().active_look()
    }

    /// См. [`LookService::active_settings`].
    pub fn active_settings(&self) -> LookSettings {
        self.lock().active_settings()
    }

    /// См. [`LookService::drain_save_outcomes`].
    pub fn drain_save_outcomes(&self) -> Vec<SaveOutcome> {
        self.lock().drain_save_outcomes()
    }

    /// См. [`LookService::startup`].
    pub fn startup(&self) -> LookStartup {
        self.lock().startup()
    }
}

#[cfg(test)]
#[path = "look_service_tests.rs"]
mod tests;
