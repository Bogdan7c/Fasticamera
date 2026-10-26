//! Запуск движка камеры: путь «конфиг → поток движка → каталог» и запасной путь при отказе.

use camera_core::AppPresence;

use super::*;
use crate::camera_test_support::{FakeCameraRig, WAIT};

#[test]
fn started_engine_reads_the_catalog_and_begins_in_background() {
    let rig = FakeCameraRig::new();
    let status = rig
        .handle()
        .wait_until_status(WAIT, |status| !status.catalog.cameras().is_empty())
        .expect("каталог прочитан");
    assert_eq!(status.presence, AppPresence::Background);
    assert_eq!(
        status.catalog.cameras().len(),
        4,
        "видимые камеры OnePlus 11: 0, 1, 2, 3"
    );
    assert!(
        rig.control
            .calls()
            .contains(&camera_core::fake::BackendCall::LoadCatalog),
        "каталог прочитан бэкендом"
    );
    assert!(!rig.control.is_device_open(), "продукт камеру сам не открывает");
}

#[test]
fn backend_diagnostics_and_reader_limit_reach_the_service() {
    let mut config = CameraConfig::default();
    config.android.raw_reader_max_images = 7;
    let rig = FakeCameraRig::with_config(config);
    rig.diagnostics.raw_sink().record_frame_acquired();
    let running = rig.services.running().expect("запущен");
    assert_eq!(running.raw_reader_max_images(), 7);
    assert_eq!(running.diagnostics_snapshot().raw_sink.frames_acquired, 1);
}

#[test]
fn invalid_config_gives_unavailable_service_with_the_reason_and_no_thread() {
    let mut config = CameraConfig::default();
    config.engine.recovery.max_consecutive_attempts = 0;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
        },
    );
    let error = services.running().err().expect("камера недоступна");
    assert!(error.reason.contains("неверные настройки камеры"), "{error}");
    assert!(error.reason.contains("max_consecutive_attempts"), "{error}");
    assert!(control.calls().is_empty(), "бэкенд не создавался и не вызывался");
}
