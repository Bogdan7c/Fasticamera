//! Запуск движка камеры и сборка [`CameraServices`] (по образцу `render_wiring`).
//!
//! Модуль нейтрален к платформе: фабрика бэкенда и источник диагностики приходят снаружи (на
//! телефоне — Camera2 NDK из `camera-android`, в тестах — fake `camera-core`), поэтому путь
//! «событие жизненного цикла → команда → бэкенд» проверяется на ПК настоящим потоком движка.
//!
//! Движок стартует с присутствием `Background`: Android отдаёт камеру только приложению на переднем
//! плане, а событие `Resumed` маршрутизатора переведёт движок в `Foreground`. Сразу после запуска
//! движка ставится желаемое состояние живого видоискателя (временная политика до сессии 12,
//! `viewfinder_autostart`, конфиг `camera.viewfinder_autostart`): в фоне это только желаемое,
//! камера откроется по `Resumed`. Маршрутизатор об автозапуске не знает.

use std::sync::Arc;

use camera_core::backend::BackendFactory;
use camera_core::{AppPresence, CameraRuntime};

use crate::camera_config::CameraConfig;
use crate::camera_service::{BackendDiagnosticsSource, CameraEngineAccess, CameraServices, RunningCamera};
use crate::raw_stream_services::RawStreamServices;
use crate::viewfinder_autostart::ViewfinderAutostart;

const LOG_TARGET: &str = "fasticamera::camera::wiring";

/// То, что платформа даёт движку: как создать бэкенд и откуда читать его диагностику.
pub struct CameraBackendParts {
    /// Фабрика бэкенда: вызывается один раз на потоке камеры.
    pub factory: BackendFactory,
    pub diagnostics: Arc<dyn BackendDiagnosticsSource>,
    /// Наблюдательные сервисы потока кадров, который бэкенд наполняет (счётчики, проба CPU, часы).
    pub raw_stream: RawStreamServices,
}

/// Запускает движок камеры. Любой отказ запуска (неверный конфиг, не создавшийся поток) не роняет
/// приложение: возвращается сервис «камера недоступна: причина».
pub fn start_camera(config: &CameraConfig, backend: CameraBackendParts) -> CameraServices {
    let services = spawn_engine(config, backend);
    start_product_viewfinder(services, config)
}

/// Запуск потока движка; отказ — сервис «камера недоступна: причина».
fn spawn_engine(config: &CameraConfig, backend: CameraBackendParts) -> CameraServices {
    if let Err(error) = config.validate() {
        return CameraServices::unavailable(format!("неверные настройки камеры: {error}"), config.scenarios.clone());
    }
    let CameraBackendParts {
        factory,
        diagnostics,
        raw_stream,
    } = backend;
    match CameraRuntime::spawn(config.engine.clone(), AppPresence::Background, factory) {
        Ok((runtime, handle)) => {
            tracing::info!(target: LOG_TARGET, "движок камеры запущен, присутствие: фон до события Resumed");
            let running = RunningCamera::new(runtime, handle, diagnostics);
            CameraServices::new(CameraEngineAccess::Running(Arc::new(running)), config.scenarios.clone())
                .with_raw_stream(raw_stream)
        }
        Err(error) => {
            CameraServices::unavailable(format!("движок камеры не запущен: {error}"), config.scenarios.clone())
        }
    }
}

/// Подключает автозапуск видоискателя и выставляет его желаемое состояние. При недоступной камере
/// автозапуск тоже подключается (если его секция верна): в логе честно «камера недоступна:
/// видоискатель не открыт», а не тишина. Итог уже записан в лог владельцем политики.
fn start_product_viewfinder(services: CameraServices, config: &CameraConfig) -> CameraServices {
    let autostart = match ViewfinderAutostart::from_config(&config.viewfinder_autostart) {
        Ok(autostart) => autostart,
        Err(error) => {
            // Сюда попадаем только с уже недоступной камерой: `spawn_engine` проверил весь конфиг.
            tracing::error!(target: LOG_TARGET, %error, "автозапуск видоискателя не подключён");
            ViewfinderAutostart::disabled()
        }
    };
    let services = services.with_viewfinder_autostart(autostart);
    let _logged_by_owner = services.start_product_viewfinder();
    services
}

#[cfg(test)]
#[path = "camera_wiring_tests.rs"]
mod tests;
