//! Запуск движка камеры и сборка [`CameraServices`] (по образцу `render_wiring`).
//!
//! Модуль нейтрален к платформе: фабрика бэкенда и источник диагностики приходят снаружи (на
//! телефоне — Camera2 NDK из `camera-android`, в тестах — fake `camera-core`), поэтому путь
//! «событие жизненного цикла → команда → бэкенд» проверяется на ПК настоящим потоком движка.
//!
//! Движок стартует с присутствием `Background`: Android отдаёт камеру только приложению на переднем
//! плане, а событие `Resumed` маршрутизатора переведёт движок в `Foreground`. Продукт камеру сам не
//! открывает (видоискателя ещё нет) — её открывают только сценарии.

use std::sync::Arc;

use camera_core::backend::BackendFactory;
use camera_core::{AppPresence, CameraRuntime};

use crate::camera_config::CameraConfig;
use crate::camera_service::{BackendDiagnosticsSource, CameraEngineAccess, CameraServices, RunningCamera};

const LOG_TARGET: &str = "fasticamera::camera::wiring";

/// То, что платформа даёт движку: как создать бэкенд и откуда читать его диагностику.
pub struct CameraBackendParts {
    /// Фабрика бэкенда: вызывается один раз на потоке камеры.
    pub factory: BackendFactory,
    pub diagnostics: Arc<dyn BackendDiagnosticsSource>,
}

/// Запускает движок камеры. Любой отказ запуска (неверный конфиг, не создавшийся поток) не роняет
/// приложение: возвращается сервис «камера недоступна: причина».
pub fn start_camera(config: &CameraConfig, backend: CameraBackendParts) -> CameraServices {
    if let Err(error) = config.validate() {
        return CameraServices::unavailable(format!("неверные настройки камеры: {error}"), config.scenarios.clone());
    }
    let CameraBackendParts { factory, diagnostics } = backend;
    match CameraRuntime::spawn(config.engine.clone(), AppPresence::Background, factory) {
        Ok((runtime, handle)) => {
            tracing::info!(target: LOG_TARGET, "движок камеры запущен, присутствие: фон до события Resumed");
            let running = RunningCamera::new(runtime, handle, diagnostics, config.android.raw_reader_max_images);
            CameraServices::new(CameraEngineAccess::Running(Arc::new(running)), config.scenarios.clone())
        }
        Err(error) => {
            CameraServices::unavailable(format!("движок камеры не запущен: {error}"), config.scenarios.clone())
        }
    }
}

#[cfg(test)]
#[path = "camera_wiring_tests.rs"]
mod tests;
