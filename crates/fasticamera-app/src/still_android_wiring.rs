//! Подключение настоящего снимка на телефоне (только Android): собирает платформенные сервисы оболочки и камеру
//! в зависимости службы записи. Решений о съёмке здесь нет — их принимают `still_runtime` и `photo-output`.
//!
//! Любая недостающая часть (рендер, камера, паспорт телефона) — не паника и не молчаливая подмена, а явный
//! запасной фейк с причиной (`CaptureMode::FakeFallback`): приложение живёт, экран работает, диагностика честно
//! говорит, что снимки в галерею не пишутся.

use std::sync::Arc;

use android_shell::android::{
    AndroidHeifEncoderService, AndroidJpegCompressor, AndroidMediaStoreService, AndroidShellServices,
};
use android_shell::still_orphan_sweep::OrphanSweepReport;
use photo_output::service::PhotoFileFormats;

use crate::camera_service::CameraServices;
use crate::flash_camera_port::CameraHandleFlashPort;
use crate::flash_still_relay::FlashStillRelay;
use crate::runtime_config::RuntimeConfig;
use crate::still_camera_catalog::{EngineCameraCatalog, HandleCatalogSource, software_description};
use crate::still_render_port::StillRenderLink;
use crate::still_runtime::{OrphanSweepSource, StillAssembly, StillRuntimeDeps, start_still_or_fallback};
use crate::still_wall_clock::SystemWallClock;

/// Итог уборки сирот берётся из MediaStore-сервиса оболочки.
struct AndroidOrphanSweep(Arc<AndroidMediaStoreService>);

impl OrphanSweepSource for AndroidOrphanSweep {
    fn orphan_sweep_report(&self) -> OrphanSweepReport {
        self.0.orphan_sweep_report()
    }
}

/// Собирает снимок: настоящий, если есть всё нужное, иначе запасной фейк с причиной.
pub(crate) fn assemble_still(
    config: &RuntimeConfig,
    services: &Arc<AndroidShellServices>,
    render: StillRenderLink,
    camera: &CameraServices,
    flash_relay: Arc<FlashStillRelay>,
) -> StillAssembly {
    let prerequisites = gather_dependencies(config, services, render, camera, flash_relay);
    start_still_or_fallback(config.photo(), config.camera_screen(), prerequisites)
}

/// Собирает зависимости службы или называет причину, по которой настоящий снимок невозможен.
fn gather_dependencies(
    config: &RuntimeConfig,
    services: &Arc<AndroidShellServices>,
    render: StillRenderLink,
    camera: &CameraServices,
    flash_relay: Arc<FlashStillRelay>,
) -> Result<StillRuntimeDeps, String> {
    let render = match render {
        StillRenderLink::Live(port) => port,
        StillRenderLink::Unavailable { reason } => return Err(format!("рендер недоступен: {reason}")),
    };
    let running = camera
        .running()
        .map_err(|error| format!("камера недоступна: {}", error.reason))?;
    let device = config.device().identity().map_err(|error| error.to_string())?;
    let store = Arc::new(AndroidMediaStoreService::new(services));
    let cameras = EngineCameraCatalog::new(
        Arc::new(HandleCatalogSource::new(running.handle().clone())),
        device,
        software_description(),
    );
    Ok(StillRuntimeDeps {
        store: Arc::clone(&store) as Arc<dyn shell_contract::still_media::MediaStoreService>,
        heif: Arc::new(AndroidHeifEncoderService::new(services)),
        jpeg: Arc::new(AndroidJpegCompressor::new(services)),
        formats: Arc::new(PhotoFileFormats),
        clock: Arc::new(SystemWallClock::default()),
        cameras: Arc::new(cameras),
        render,
        shutter_sound: Arc::clone(services) as Arc<dyn shell_contract::still_media::ShutterSoundService>,
        orphan_sweep: Arc::new(AndroidOrphanSweep(store)),
        flash_camera: Arc::new(CameraHandleFlashPort::new(running.handle().clone())),
        flash_relay,
    })
}
