//! `android-shell` — шов Android ↔ Rust (сессия 02).
//!
//! Единственный crate, который знает `jni` и NDK (guardrail `jni-ndk-only-in-shell`). Наружу
//! отдаёт только нейтральные типы `shell-contract`: события в очереди и реализации трейтов
//! команд, хаптики, датчиков, тепла и диагностики. Бизнес-логики здесь нет.
//!
//! Раскладка:
//! * нейтральная часть (собирается и тестируется на ПК): [`config`], [`touch_decode`],
//!   [`haptic_plan`], [`permission_answer`], [`orientation_classifier`], [`surface_registry`],
//!   [`accounting`], [`thermal_codes`], [`command_codes`], [`vsync_timeline`];
//! * только Android: модуль `android` — регистрация JNI-методов, safety-модули FFI
//!   (`ANativeWindow`, `AMotionEvent`, датчики, тепло) и поток команд к Kotlin.
//!
//! Контракт времени жизни surface: `surfaceDestroyed` ждёт подтверждения владельца рендера
//! (`SurfaceReleaseRequest`) не дольше `surface_release_timeout_ms`; таймаут — ошибка в логе
//! и счётчик `surface_release_timeouts`, а не зависание главного потока.

pub mod accounting;
pub mod command_codes;
pub mod config;
pub mod haptic_plan;
pub mod orientation_classifier;
pub mod permission_answer;
pub mod surface_registry;
pub mod thermal_codes;
pub mod touch_decode;
pub mod vsync_timeline;

#[cfg(target_os = "android")]
pub mod android;
