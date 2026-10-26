//! `camera-android` — бэкенд камеры Fasticamera на Camera2 NDK.
//!
//! Реализует трейт `camera_core::backend::CameraBackend` для телефона. Устройство crate-а:
//! * **нейтральная логика** (собирается и тестируется на ПК без NDK): разбор характеристик
//!   ([`characteristics`]) и результатов ([`result_meta`]) через трейт-источник метаданных
//!   ([`entry`]), реестр вендорских тегов ([`vendor`]), выбор рычага частоты ([`frame_rate_plan`]),
//!   план записей запроса ([`request_plan`]), коды NDK → смысловые отказы ([`status_mapping`]),
//!   сопоставление номеров кадров ([`frame_numbers`]), диагностика ([`diagnostics`]);
//! * **тонкий слой NDK** (`android`, только под Android): RAII-владельцы объектов Camera2,
//!   колбэки `extern "C"` в двух safety-модулях, временный RAW-приёмник и сам бэкенд.
//!
//! Инварианты:
//! * вендорские ключи знает только [`vendor`]; наружу уходит смысл рычага (`FrameRateMechanism`);
//! * ни один колбэк NDK не блокируется и не паникует через границу C;
//! * ни один метод бэкенда не паникует на отказе платформы — отказ приходит вариантом.

pub mod callback_gate;
pub mod characteristics;
pub mod config;
pub mod context_leak;
pub mod diagnostics;
pub mod entry;
pub mod frame_numbers;
pub mod frame_rate_plan;
pub mod reader_drain;
pub mod request_plan;
pub mod result_meta;
pub mod status_mapping;
pub mod tags;
pub mod vendor;

#[cfg(test)]
mod fake_metadata;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "android")]
pub use android::{AndroidBackendParts, backend_factory};
pub use config::{AndroidCameraConfig, ConfigError};
pub use diagnostics::{BackendDiagnostics, DiagnosticsSnapshot};
