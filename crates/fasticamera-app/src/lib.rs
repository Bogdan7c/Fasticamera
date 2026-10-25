//! `fasticamera-app` — composition root Fasticamera: нативная библиотека `libfasticamera.so`.
//!
//! Сейчас (сессия 01) это каркас: точка входа JNI, логирование в logcat, перехват паник
//! и smoke-сценарии для `scripts/device-smoke.sh`. Логики фич здесь нет и не будет:
//! crate только собирает модули и маршрутизирует вызовы (README плана, «Владельцы состояния»).
//!
//! Раскладка:
//! * платформенно-нейтральная часть (собирается и тестируется на ПК): `runtime_config`,
//!   `build_info`, `logging` (кроме приёмника logcat), `panic_report`, `jni_boundary`,
//!   `scenario`, `result_line`, `runtime`;
//! * только Android: `jni_exports` и приёмник `logging::logcat_sink`.

pub mod build_info;
pub mod jni_boundary;
pub mod logging;
pub mod panic_report;
pub mod result_line;
pub mod runtime;
pub mod runtime_config;
pub mod scenario;

#[cfg(target_os = "android")]
mod jni_exports;
