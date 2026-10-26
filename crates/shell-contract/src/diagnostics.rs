//! Счётчики шва: доказательства для smoke-сценариев и диагностики (утечки, потери, таймауты).

use serde::Serialize;

/// Снимок счётчиков оболочки с начала процесса.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ShellCounters {
    /// Живые глобальные ссылки JNI, созданные оболочкой (создано − удалено). Рост между
    /// одинаковыми состояниями — утечка.
    pub live_global_refs: u64,
    /// Сколько раз Activity подключалась к оболочке (каждый `onCreate`).
    pub activity_attachments: u64,
    /// Сколько раз владелец рендера не подтвердил освобождение surface за отведённое время.
    pub surface_release_timeouts: u64,
    /// Сколько касаний выброшено из-за переполнения очереди.
    pub touch_samples_dropped: u64,
    /// Сколько событий жизненного цикла и сервисов не поместилось в очередь (не должно быть никогда).
    pub critical_events_lost: u64,
    /// Сколько команд платформы не выполнилось (отказы пришли событиями).
    pub platform_command_failures: u64,
    /// Сколько колбэков из Kotlin завершились ошибкой или паникой (залогированы).
    pub native_callback_failures: u64,
}

/// Источник счётчиков (оболочка или fake в тестах).
pub trait ShellDiagnostics: Send + Sync {
    fn counters(&self) -> ShellCounters;
}
