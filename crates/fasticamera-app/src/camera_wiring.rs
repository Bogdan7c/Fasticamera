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

use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective};
use camera_core::backend::BackendFactory;
use camera_core::{AppPresence, CameraRuntime, ThreeAWiring};

use crate::auto_exposure_link::{
    AutoExposureLink, AutoExposureRenderEnd, ExposureControllerChoice, ExposureControllerKind, RenderAutoExposure,
};
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

/// Ящики, которыми движок камеры связан с потоком рендера. Корень композиции создаёт их один раз на
/// процесс: одна половина уходит движку, такой же клон — конструктору бэкенда рендера.
#[derive(Debug, Clone, Default)]
pub struct CameraLinks {
    /// Указания баланса белого (решение 64): движок пишет, рендер читает.
    pub white_balance: DirectiveSlot<WhiteBalanceDirective>,
    /// Предложения и указания нашего AE (08a); с вендорским AE не используются.
    pub auto_exposure: AutoExposureLink,
    /// Поднялся ли рендер, который будет мерить сцену для нашего AE (решает, какой AE получит камера).
    pub render_auto_exposure: RenderAutoExposure,
}

/// Рендерная половина тех же ящиков ([`CameraLinks::render_side`]).
#[derive(Debug, Clone)]
pub struct RenderLinks {
    /// Читатель указаний баланса белого.
    pub white_balance: DirectiveSlot<WhiteBalanceDirective>,
    /// Сторона рендера нашего AE; `None`, если у камеры вендорский AE.
    pub auto_exposure: Option<AutoExposureRenderEnd>,
}

impl CameraLinks {
    /// Половина для потока рендера: клоны тех же ящиков, что держит движок.
    pub fn render_side(&self, exposure: ExposureControllerKind) -> RenderLinks {
        RenderLinks {
            white_balance: self.white_balance.clone(),
            auto_exposure: self.auto_exposure.render_end(exposure),
        }
    }
}

/// Запускает движок камеры. Любой отказ запуска (неверный конфиг, не создавшийся поток) не роняет
/// приложение: возвращается сервис «камера недоступна: причина».
///
/// Ящики связи здесь свои и никем не читаются: так запускают тесты и стенды без рендера. Боевой корень
/// композиции зовёт [`start_camera_with_links`].
pub fn start_camera(config: &CameraConfig, backend: CameraBackendParts) -> CameraServices {
    start_camera_with_links(config, backend, CameraLinks::default())
}

/// Запускает движок камеры, отдавая ему **камерную** половину ящиков связи с рендером (решения 64, 08a).
///
/// Ни движок, ни рендер не знают друг о друге: между ними только нейтральные ящики из `camera-contract`.
pub fn start_camera_with_links(
    config: &CameraConfig,
    backend: CameraBackendParts,
    links: CameraLinks,
) -> CameraServices {
    let services = spawn_engine(config, backend, links);
    start_product_viewfinder(services, config)
}

/// Состав 3A по переключателю `camera.exposure_controller`: наш AE (с ящиками связи) или вендорский.
fn three_a_wiring(config: &CameraConfig, links: &CameraLinks, effective: ExposureControllerKind) -> ThreeAWiring {
    let wiring = match effective {
        ExposureControllerKind::Own => ThreeAWiring::own(
            &config.engine.three_a,
            links.auto_exposure.engine_proposals(),
            config.android.mains_frequency,
        )
        .with_exposure_directive_slot(links.auto_exposure.engine_directive_slot()),
        ExposureControllerKind::Vendor => ThreeAWiring::vendor(&config.engine.three_a),
    };
    wiring.with_white_balance_slot(links.white_balance.clone())
}

/// Запуск потока движка; отказ — сервис «камера недоступна: причина».
fn spawn_engine(config: &CameraConfig, backend: CameraBackendParts, links: CameraLinks) -> CameraServices {
    if let Err(error) = config.validate() {
        return CameraServices::unavailable(format!("неверные настройки камеры: {error}"), config.scenarios.clone());
    }
    let CameraBackendParts {
        factory,
        diagnostics,
        raw_stream,
    } = backend;
    let choice = ExposureControllerChoice::resolve(config.exposure_controller, &links.render_auto_exposure);
    if let Some(reason) = &choice.fallback_reason {
        tracing::warn!(
            target: LOG_TARGET,
            configured = ?choice.configured,
            effective = ?choice.effective,
            %reason,
            "исполнитель экспозиции заменён"
        );
    }
    let wiring = three_a_wiring(config, &links, choice.effective);
    match CameraRuntime::spawn_with_wiring(config.engine.clone(), AppPresence::Background, factory, wiring) {
        Ok((runtime, handle)) => {
            tracing::info!(
                target: LOG_TARGET,
                exposure_controller = ?choice.effective,
                "движок камеры запущен, присутствие: фон до события Resumed"
            );
            let mut running = RunningCamera::new(runtime, handle, diagnostics);
            if choice.effective == ExposureControllerKind::Own {
                // Толчок рендера привязываем только после запуска: `ThreeAWaker` есть лишь у живого движка.
                // Предложение, опубликованное раньше, не потеряно: оно лежит в ящике, движок прочтёт его
                // при первом же плане.
                let waker = running.handle().three_a_waker();
                running = running.with_exposure_binding(links.auto_exposure.bind_engine(Arc::new(waker)));
            }
            CameraServices::new(CameraEngineAccess::Running(Arc::new(running)), config.scenarios.clone())
                .with_raw_stream(raw_stream)
                .with_exposure_controller(choice)
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
