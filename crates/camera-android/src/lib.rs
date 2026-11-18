//! `camera-android` — бэкенд камеры Fasticamera на Camera2 NDK.
//!
//! Реализует трейт `camera_core::backend::CameraBackend` для телефона. Устройство crate-а:
//! * **нейтральная логика** (собирается и тестируется на ПК без NDK): разбор характеристик
//!   ([`characteristics`]) и результатов ([`result_meta`]) через трейт-источник метаданных
//!   ([`entry`]), реестр вендорских тегов ([`vendor`]), выбор рычага частоты ([`frame_rate_plan`]),
//!   план записей запроса ([`request_plan`]), коды NDK → смысловые отказы ([`status_mapping`]),
//!   сопоставление номеров кадров ([`frame_numbers`]), паспорт потока ([`stream_passport`]),
//!   слияние fence-ов ([`fence_fold`]), опустошение reader-а ([`reader_drain`]), отсрочка удаления
//!   reader-ов ([`retired_readers`]), диагностика ([`diagnostics`]);
//! * **тонкий слой NDK** (`android`, только под Android): RAII-владельцы объектов Camera2,
//!   колбэки `extern "C"` в двух safety-модулях, reader с лизингом кадров и сам бэкенд.
//!
//! Инварианты:
//! * вендорские ключи знает только [`vendor`]; наружу уходит смысл рычага (`FrameRateMechanism`);
//! * ни один колбэк NDK не блокируется и не паникует через границу C;
//! * ни один метод бэкенда не паникует на отказе платформы — отказ приходит вариантом.

pub mod callback_gate;
pub mod characteristics;
pub mod companion_negotiation;
pub mod companion_stream;
pub mod config;
pub mod context_leak;
pub mod cpu_access_probe;
pub mod diagnostics;
pub mod entry;
pub mod fence_fold;
pub mod frame_numbers;
pub mod frame_rate_plan;
pub mod metering_region;
pub mod outstanding_frames;
pub mod reader_counters;
pub mod reader_drain;
pub mod request_controls_plan;
pub mod request_origin;
pub mod request_plan;
pub mod result_meta;
pub mod retired_readers;
pub mod status_mapping;
pub mod stream_passport;
pub mod tags;
mod three_a_characteristics;
pub mod three_a_result;
pub mod vendor;

#[cfg(test)]
mod fake_metadata;

#[cfg(target_os = "android")]
mod android;

#[cfg(target_os = "android")]
pub use android::{AndroidBackendParts, backend_factory};
pub use config::{AndroidCameraConfig, ConfigError};
pub use cpu_access_probe::{CpuAccessProbe, CpuProbeMode};
pub use diagnostics::{BackendDiagnostics, DiagnosticsSnapshot};
