//! Перехват паник: место и сообщение — в logcat, отчёт — вызывающему через границу JNI.
//!
//! Решение владельца (сессия 01): паника ловится на границе Rust ↔ Kotlin и превращается
//! в отчёт, а не роняет процесс. Хук срабатывает на паникующем потоке до раскрутки стека:
//! он пишет запись уровня FATAL мимо фильтра логов и кладёт отчёт в слот потока, откуда его
//! забирает `jni_boundary::run_guarded`.

use std::cell::RefCell;
use std::panic::PanicHookInfo;
use std::sync::Once;

use serde::Serialize;

use crate::logging::{LogPriority, emit_unfiltered};

/// Что известно о панике.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PanicReport {
    /// Текст `panic!(...)`; для нетекстового payload — пометка об этом.
    pub message: String,
    /// `файл:строка:столбец`; `None`, если std места не сообщила.
    pub location: Option<String>,
    /// Имя потока или `<без имени>`.
    pub thread: String,
}

impl PanicReport {
    fn from_hook_info(info: &PanicHookInfo<'_>) -> Self {
        let message = info
            .payload_as_str()
            .map_or_else(|| NON_TEXT_PAYLOAD.to_owned(), str::to_owned);
        let location = info
            .location()
            .map(|location| format!("{}:{}:{}", location.file(), location.line(), location.column()));
        Self {
            message,
            location,
            thread: current_thread_name(),
        }
    }

    /// Отчёт по payload из `catch_unwind`, когда хук не успел записать свой (например, его
    /// подменил чужой код). Места в payload нет — это честно видно как `location: None`.
    pub fn from_payload(payload: &(dyn std::any::Any + Send)) -> Self {
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| NON_TEXT_PAYLOAD.to_owned());
        Self {
            message,
            location: None,
            thread: current_thread_name(),
        }
    }

    /// Строка для logcat: `panic at <место> (thread <имя>): <сообщение>`.
    pub fn log_text(&self) -> String {
        let location = self.location.as_deref().unwrap_or("<место неизвестно>");
        format!("panic at {location} (thread {}): {}", self.thread, self.message)
    }
}

const NON_TEXT_PAYLOAD: &str = "<payload паники не текстовый>";

fn current_thread_name() -> String {
    std::thread::current().name().unwrap_or("<без имени>").to_owned()
}

thread_local! {
    /// Последняя паника этого потока, ещё не забранная границей.
    // Инициализатор уже `const`; clippy 1.97 ложно ругается только для aarch64-linux-android
    // (минимальный пример — отчёт сессии 02). `expect` сам станет ошибкой, когда это починят.
    #[cfg_attr(
        target_os = "android",
        expect(
            clippy::missing_const_for_thread_local,
            reason = "ложное срабатывание clippy 1.97 на aarch64-linux-android: инициализатор уже const"
        )
    )]
    static LAST_PANIC: RefCell<Option<PanicReport>> = const { RefCell::new(None) };
}

static INSTALL_HOOK: Once = Once::new();

/// Ставит хук паник один раз на процесс. Повторные вызовы — no-op.
pub fn install_panic_hook() {
    INSTALL_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let report = PanicReport::from_hook_info(info);
            emit_unfiltered(LogPriority::Fatal, report.log_text());
            LAST_PANIC.with(|slot| *slot.borrow_mut() = Some(report));
        }));
    });
}

/// Забирает отчёт о последней панике этого потока (слот очищается).
pub fn take_last_panic_on_this_thread() -> Option<PanicReport> {
    LAST_PANIC.with(|slot| slot.borrow_mut().take())
}
