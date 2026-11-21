//! Слой Camera2 NDK (только Android): RAII-владельцы объектов NDK, колбэки и сам бэкенд.
//!
//! Модули:
//! * `callbacks` и `reader_callback` — единственные места с `extern "C"` (guardrail
//!   `camera-ffi-callbacks-only-in-safety-modules`);
//! * `manager`, `device`, `session`, `outputs`, `request`, `raw_reader`, `companion` (вспомогательный
//!   поток 3A), `availability` — владельцы объектов NDK;
//! * `producer_frame` — кадр потока (`AImage`) и его возврат камере с fence-ом;
//! * `standby_slot` — ячейки тёплых камер (13b); `backend/standby_api` — их `StandbyCamera` и обмен ролей;
//! * `backend` — `AndroidCameraBackend`.
//!
//! Весь `unsafe` здесь сопровождён `// SAFETY:`; решения (какой рычаг, какие записи, какой отказ)
//! принимает нейтральный код crate-а и проверяет тестами на ПК.

mod availability;
mod backend;
mod boottime_clock;
mod callbacks;
mod companion;
mod device;
mod manager;
mod outputs;
mod producer_frame;
mod raw_reader;
mod reader_callback;
mod request;
mod session;
mod standby_slot;

use std::sync::Arc;

use camera_core::backend::{BackendEventSink, BackendFactory};

use raw_frame_stream::RawFrameProducer;

use crate::config::AndroidCameraConfig;
use crate::cpu_access_probe::CpuAccessProbe;
use crate::diagnostics::BackendDiagnostics;

pub use backend::AndroidCameraBackend;

/// Фабрика бэкенда и ручка диагностики, которую приложение может читать снимком.
pub struct AndroidBackendParts {
    /// Фабрика для потока камеры (`camera_core`): вызывается один раз на этом потоке.
    pub factory: BackendFactory,
    /// Общая диагностика бэкенда (счётчики reader-ов, вендорские теги, отказы).
    pub diagnostics: Arc<BackendDiagnostics>,
    /// Рубильник отладочной пробы CPU-доступа (по умолчанию выключена). Включать до
    /// `configure_session`: reader создаётся с usage `CPU_READ_RARELY` только при включённой пробе.
    pub cpu_access_probe: CpuAccessProbe,
}

/// Строит фабрику бэкенда на Camera2 NDK. Менеджер Camera2 создаётся **на потоке камеры**, внутри
/// вызова фабрики; неверный конфиг приводит к ошибке фабрики, а не к панике.
///
/// `producer` — вход потока кадров (`RawFrameStream::producer()`): через него камера отдаёт
/// кадры в лизинг и сообщает результаты запросов.
pub fn backend_factory(config: AndroidCameraConfig, producer: RawFrameProducer) -> AndroidBackendParts {
    let diagnostics = Arc::new(BackendDiagnostics::new(config.failure_history_capacity));
    let backend_diagnostics = Arc::clone(&diagnostics);
    let cpu_access_probe = CpuAccessProbe::disarmed();
    let backend_probe = cpu_access_probe.clone();
    let factory: BackendFactory = Box::new(move |events: BackendEventSink| {
        let backend = AndroidCameraBackend::new(config, events, backend_diagnostics, producer, backend_probe)?;
        Ok(Box::new(backend) as Box<dyn camera_core::backend::CameraBackend>)
    });
    AndroidBackendParts {
        factory,
        diagnostics,
        cpu_access_probe,
    }
}
