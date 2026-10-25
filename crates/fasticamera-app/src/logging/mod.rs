//! Логирование: события `tracing` → приёмник платформы (logcat на Android, stderr на ПК).
//!
//! Свой маленький слой поверх зрелых `tracing`/`tracing-subscriber` и `android_log-sys`
//! (решение владельца, сессия 01: готовые мосты в logcat молодые и с одним автором).
//!
//! Устройство:
//! * `record` — чистая логика: уровень logcat, текст события, нарезка длинных строк;
//! * `sink` — куда пишется готовая запись (трейт `LogSink` и приёмник платформы);
//! * `layer` — слой tracing, который превращает событие в запись и отдаёт приёмнику;
//! * `logcat_sink` — FFI к `__android_log_write` (только Android, единственный unsafe).
//!
//! Фильтр уровней приходит из конфига (`RuntimeConfig::log_filter`). Панике и строке
//! результата smoke-сценария фильтр не мешает: они пишутся напрямую через `emit_unfiltered`.

mod layer;
#[cfg(target_os = "android")]
mod logcat_sink;
pub mod record;
pub mod sink;

use std::sync::OnceLock;

use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

pub use layer::SinkLayer;
pub use record::{LogPriority, LogRecord};
pub use sink::{LogSink, PlatformSink, platform_write_failures};

use crate::runtime_config::LogFilter;

/// Тег logcat всех нативных логов. Это контракт со скриптами (`scripts/device-*.sh`
/// фильтруют logcat по нему), поэтому константа, а не настройка.
pub const LOGCAT_TAG: &str = "Fasticamera";

/// Тот же тег в виде C-строки для FFI (`logcat_sink`). Совпадение с `LOGCAT_TAG` проверяет
/// тест ниже — он живёт здесь, а не в android-only модуле, чтобы запускаться на ПК.
pub const LOGCAT_TAG_C: &std::ffi::CStr = c"Fasticamera";

/// Итог единственной попытки поставить глобального подписчика в этом процессе:
/// действующий фильтр или текст ошибки `set_global_default`.
static GLOBAL_INSTALL: OnceLock<Result<LogFilter, String>> = OnceLock::new();

/// Итог установки глобального логирования.
#[derive(Debug, Clone, PartialEq)]
pub enum LoggingInstall {
    /// Подписчик установлен этим вызовом.
    Installed,
    /// Подписчик уже стоял (процесс переиспользован: Activity создана повторно).
    /// Фильтр поменять нельзя — он фиксирован на процесс; здесь тот, что действует.
    AlreadyInstalled { active_filter: LogFilter },
}

/// Почему логирование не установлено.
#[derive(Debug, thiserror::Error)]
pub enum LoggingError {
    #[error("глобальный подписчик tracing уже установлен не нами: {0}")]
    ForeignSubscriber(String),
}

/// Собирает подписчика: реестр tracing + наш слой с фильтром. Отдельно от установки,
/// чтобы тесты проверяли настоящий конвейер на локальном подписчике, не трогая глобальный.
pub fn build_subscriber<Sink: LogSink>(filter: &LogFilter, sink: Sink) -> impl Subscriber + Send + Sync {
    tracing_subscriber::registry().with(SinkLayer::new(sink).with_filter(filter.targets().clone()))
}

/// Ставит глобального подписчика с приёмником платформы. Повторный вызов в том же процессе —
/// не ошибка, а `AlreadyInstalled` с действующим фильтром.
pub fn install_global_logging(filter: &LogFilter) -> Result<LoggingInstall, LoggingError> {
    // Проверка «уже стоит?» и установка — одна атомарная операция: `get_or_init` держит
    // остальные потоки, пока первый не закончит. Раздельные проверка и `set_global_default`
    // давали гонку: второй поток видел «ещё не стоит», проигрывал установку и ложно
    // сообщал о чужом подписчике.
    let mut installed_by_this_call = false;
    let outcome = GLOBAL_INSTALL.get_or_init(|| {
        installed_by_this_call = true;
        tracing::subscriber::set_global_default(build_subscriber(filter, PlatformSink))
            .map(|()| filter.clone())
            .map_err(|error| error.to_string())
    });
    match outcome {
        Ok(_) if installed_by_this_call => Ok(LoggingInstall::Installed),
        Ok(active_filter) => Ok(LoggingInstall::AlreadyInstalled {
            active_filter: active_filter.clone(),
        }),
        Err(error) => Err(LoggingError::ForeignSubscriber(error.clone())),
    }
}

/// Пишет запись в приёмник платформы мимо фильтра. Только для того, что нельзя потерять
/// из-за настройки уровня: отчёт о панике и строка результата smoke-сценария.
pub fn emit_unfiltered(priority: LogPriority, text: impl Into<String>) {
    PlatformSink.write(&LogRecord::new(priority, text.into()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_tag_matches_public_tag() {
        assert_eq!(LOGCAT_TAG_C.to_str().expect("utf8"), LOGCAT_TAG);
    }
}
