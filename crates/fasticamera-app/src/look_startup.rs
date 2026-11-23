//! Запуск хранилища пресетов при старте процесса (решения 99, 179): открыть файл в папке приложения, поднять
//! поток записи и отдать общую ручку на библиотеку.
//!
//! Вызывается на потоке `initialize` ДО старта рендера: начальный вид (активный пресет, по умолчанию «Как глаз»)
//! обязан быть известен к сборке бэкенда, чтобы действовать с первого кадра видоискателя.
//!
//! Правило отказов: испорченный файл, файл новее приложения и нечитаемый файл НЕ валят запуск. Встроенные пресеты
//! работают при любом исходе, а причина не глотается: `tracing::warn!` + факт в счётчиках
//! ([`crate::look_service::LookStartup`]). Файл пользователя при этом не затирается (карантин, решение 179).
//! Валит запуск только то, что бывает лишь при кривом конфиге: он проверяется ещё в `RuntimeConfig::from_json`.

use look_presets::{
    ConfigError, LoadOutcome, PRODUCT_MIGRATIONS, PresetFileStore, PresetPersistence, PresetStoreConfig,
};

use crate::app_directory::AppDirectory;
use crate::look_service::{LookHandle, LookPersistence, LookService, LookStartup, NoLookPersistence};

const LOG_TARGET: &str = "fasticamera::look";

/// Что вернул запуск: ручка на библиотеку и как прошла загрузка файла.
pub struct LookOpened {
    /// Общая ручка на библиотеку (для рендера и исполнителей эффектов).
    pub handle: LookHandle,
    /// Как прошла загрузка файла (то же самое лежит в счётчиках исполнителя).
    pub startup: LookStartup,
}

/// Открывает хранилище пресетов в папке приложения.
///
/// Не удалось поднять поток записи (ОС отказала в потоке) — работаем без записи: вид живёт до конца процесса,
/// отказ громко в логе и виден как `closed` в счётчиках просьб записи. Это не причина терять экран камеры.
pub fn open_look_service(directory: &AppDirectory, config: &PresetStoreConfig) -> Result<LookOpened, ConfigError> {
    let opened = PresetFileStore::open(directory.path(), config, PRODUCT_MIGRATIONS)?;
    let startup = LookStartup::of_outcome(&opened.outcome);
    log_load_outcome(&opened.outcome, opened.store.path());

    let persistence: Box<dyn LookPersistence> = match PresetPersistence::spawn(
        opened.store,
        config.outcome_queue_capacity,
    ) {
        Ok(persistence) => Box::new(persistence),
        Err(error) => {
            tracing::error!(target: LOG_TARGET, %error, "поток записи пресетов не поднялся: пресеты только в памяти");
            Box::new(NoLookPersistence)
        }
    };
    let handle = LookHandle::new(LookService::new(opened.library, persistence, startup));
    Ok(LookOpened { handle, startup })
}

/// Всё, кроме нормальной загрузки, оставляет след в логе: владелец должен узнать, что с его файлом.
fn log_load_outcome(outcome: &LoadOutcome, path: &std::path::Path) {
    match outcome {
        LoadOutcome::Missing => tracing::info!(target: LOG_TARGET, ?path, "файла пресетов ещё нет: встроенные пресеты"),
        LoadOutcome::Loaded { migrated_from } => {
            tracing::info!(target: LOG_TARGET, ?path, ?migrated_from, "файл пресетов загружен");
        }
        LoadOutcome::Quarantined { reason, moved_to } => {
            tracing::warn!(target: LOG_TARGET, %reason, ?moved_to, "файл пресетов испорчен: убран в карантин, работают встроенные");
        }
        LoadOutcome::QuarantineFailed { reason, failure } => {
            tracing::warn!(target: LOG_TARGET, %reason, %failure, "файл пресетов испорчен и не убран в карантин: сохранения отключены");
        }
        LoadOutcome::Unreadable { failure } => {
            tracing::warn!(target: LOG_TARGET, %failure, "файл пресетов не прочитан: сохранения отключены");
        }
    }
}

#[cfg(test)]
#[path = "look_startup_tests.rs"]
mod tests;
