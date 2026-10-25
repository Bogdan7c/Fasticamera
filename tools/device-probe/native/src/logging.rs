//! Инициализация логов: `log` → logcat с тегом `FasticameraProbeNative`, паники — в лог.

use std::sync::Once;

use android_logger::Config;
use log::LevelFilter;

/// Тег logcat.
const LOG_TAG: &str = "FasticameraProbeNative";

static INIT: Once = Once::new();

/// Идемпотентная инициализация; вызывается из каждой JNI-функции.
pub fn ensure_initialized() {
    INIT.call_once(|| {
        android_logger::init_once(Config::default().with_tag(LOG_TAG).with_max_level(LevelFilter::Debug));
        // Паника внутри catch_unwind иначе потерялась бы: stderr на Android не всегда виден.
        std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
    });
}
