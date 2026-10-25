//! Приёмник logcat: единственное место с FFI в логировании.

use std::ffi::c_int;
use std::sync::atomic::{AtomicU64, Ordering};

use super::LOGCAT_TAG_C;
use super::record::{LOGCAT_MAX_TEXT_BYTES, LogRecord, to_c_text};
use super::sink::LogSink;

/// Счётчик записей, которые logcat отверг (`__android_log_write` < 0).
static WRITE_FAILURES: AtomicU64 = AtomicU64::new(0);

/// Сколько записей logcat отверг с начала процесса.
pub fn write_failures() -> u64 {
    WRITE_FAILURES.load(Ordering::Relaxed)
}

/// Пишет записи в logcat с тегом `Fasticamera`; длинные — несколькими строками.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogcatSink;

impl LogSink for LogcatSink {
    fn write(&self, record: &LogRecord) {
        for chunk in record.chunks(LOGCAT_MAX_TEXT_BYTES) {
            let text = to_c_text(chunk);
            // SAFETY: `LOGCAT_TAG_C` и `text` — валидные NUL-терминированные строки, живут дольше
            // вызова; liblog копирует текст и потокобезопасна (документация <android/log.h>).
            let status = unsafe {
                android_log_sys::__android_log_write(record.priority as c_int, LOGCAT_TAG_C.as_ptr(), text.as_ptr())
            };
            if status < 0 {
                WRITE_FAILURES.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
