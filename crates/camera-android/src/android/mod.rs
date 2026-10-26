//! Слой Camera2 NDK (только Android): RAII-владельцы объектов NDK, колбэки и сам бэкенд.
//!
//! Модули:
//! * `callbacks` и `reader_callback` — единственные места с `extern "C"` (guardrail
//!   `camera-ffi-callbacks-only-in-safety-modules`);
//! * `manager`, `device`, `session`, `request`, `raw_sink`, `availability` — владельцы объектов NDK;
//! * `backend` — `AndroidCameraBackend`.
//!
//! Весь `unsafe` здесь сопровождён `// SAFETY:`; решения (какой рычаг, какие записи, какой отказ)
//! принимает нейтральный код crate-а и проверяет тестами на ПК.

mod availability;
mod backend;
mod callbacks;
mod device;
mod manager;
mod raw_sink;
mod reader_callback;
mod request;
mod session;

use std::sync::Arc;

use camera_core::backend::{BackendEventSink, BackendFactory};

use crate::config::AndroidCameraConfig;
use crate::diagnostics::BackendDiagnostics;

pub use backend::AndroidCameraBackend;

/// Фабрика бэкенда и ручка диагностики, которую приложение может читать снимком.
pub struct AndroidBackendParts {
    /// Фабрика для потока камеры (`camera_core`): вызывается один раз на этом потоке.
    pub factory: BackendFactory,
    /// Общая диагностика бэкенда (счётчики RAW-приёмника, вендорские теги, отказы).
    pub diagnostics: Arc<BackendDiagnostics>,
}

/// Строит фабрику бэкенда на Camera2 NDK. Менеджер Camera2 создаётся **на потоке камеры**, внутри
/// вызова фабрики; неверный конфиг приводит к ошибке фабрики, а не к панике.
pub fn backend_factory(config: AndroidCameraConfig) -> AndroidBackendParts {
    let diagnostics = Arc::new(BackendDiagnostics::new(config.failure_history_capacity));
    let backend_diagnostics = Arc::clone(&diagnostics);
    let factory: BackendFactory = Box::new(move |events: BackendEventSink| {
        let backend = AndroidCameraBackend::new(config, events, backend_diagnostics)?;
        Ok(Box::new(backend) as Box<dyn camera_core::backend::CameraBackend>)
    });
    AndroidBackendParts { factory, diagnostics }
}
