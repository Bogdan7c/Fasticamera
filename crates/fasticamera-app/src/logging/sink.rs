//! Приёмник готовых записей лога и его реализация для текущей платформы.

use super::record::LogRecord;

/// Куда пишутся записи. Реализации обязаны быть потокобезопасными: события приходят
/// с любого потока (рендер, камера, JNI).
pub trait LogSink: Send + Sync + 'static {
    fn write(&self, record: &LogRecord);
}

/// Приёмник платформы: logcat на Android.
#[cfg(target_os = "android")]
pub use super::logcat_sink::LogcatSink as PlatformSink;

/// Сколько записей приёмник платформы не смог записать (logcat вернул ошибку).
/// Ниже логов канала нет, поэтому сбой не теряется молча, а считается и попадает в отчёт `hello`.
#[cfg(target_os = "android")]
pub use super::logcat_sink::write_failures as platform_write_failures;

/// Приёмник платформы на ПК: stderr в формате, похожем на logcat (`I Fasticamera: ...`).
#[cfg(not(target_os = "android"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformSink;

#[cfg(not(target_os = "android"))]
impl LogSink for PlatformSink {
    fn write(&self, record: &LogRecord) {
        eprintln!("{} {}: {}", record.priority.letter(), super::LOGCAT_TAG, record.text);
    }
}

/// На ПК запись в stderr не возвращает ошибок, которые мы бы видели: всегда 0.
#[cfg(not(target_os = "android"))]
pub fn platform_write_failures() -> u64 {
    0
}
