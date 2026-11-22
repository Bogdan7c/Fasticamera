//! `android-shell` — шов Android ↔ Rust (сессия 02).
//!
//! Единственный crate, который знает `jni` и NDK (guardrail `jni-ndk-only-in-shell`). Наружу
//! отдаёт только нейтральные типы `shell-contract`: события в очереди и реализации трейтов
//! команд, хаптики, датчиков, тепла и диагностики. Бизнес-логики здесь нет.
//!
//! Раскладка:
//! * нейтральная часть (собирается и тестируется на ПК): [`config`], [`touch_decode`], [`key_decode`] (клавиши громкости), [`system_gesture_decode`] (зоны системных жестов),
//!   [`haptic_plan`], [`permission_answer`], [`orientation_classifier`], [`gravity_source`] (гравитация для блика: путь и фильтр), [`surface_registry`],
//!   [`accounting`], [`thermal_codes`], [`command_codes`], [`vsync_timeline`],
//!   [`battery_snapshot`] (разбор снимка батареи от Kotlin), [`power_pacing`] (сетка опроса),
//!   [`still_pending_draft`] (черновик снимка: один итог), [`still_encode_input`] (вход кодеров HEIF/JPEG), [`platform_call_error`];
//! * только Android: модуль `android` — регистрация JNI-методов, safety-модули FFI
//!   (`ANativeWindow`, `AMotionEvent`, датчики, тепло) и поток команд к Kotlin.
//!
//! Контракт времени жизни surface: `surfaceDestroyed` ждёт подтверждения владельца рендера
//! (`SurfaceReleaseRequest`) не дольше `surface_release_timeout_ms`; таймаут — ошибка в логе
//! и счётчик `surface_release_timeouts`, а не зависание главного потока.

pub mod accounting;
pub mod battery_snapshot;
pub mod command_codes;
pub mod config;
pub mod detached_worker;
pub mod gravity_source;
pub mod haptic_plan;
pub mod key_decode;
pub mod orientation_classifier;
pub mod package_name;
pub mod performance_hint_codes;
pub mod permission_answer;
pub mod platform_call_error;
pub mod power_pacing;
pub mod still_encode_input;
pub mod still_orphan_sweep;
pub mod still_pending_draft;
pub mod surface_registry;
pub mod system_gesture_decode;
pub mod thermal_codes;
pub mod touch_decode;
pub mod vsync_timeline;

#[cfg(target_os = "android")]
pub mod android;
