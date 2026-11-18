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
    assert!(
        !rig.control.is_device_open(),
        "без автозапуска видоискателя движок камеру сам не открывает"
    );
}

#[test]
fn backend_diagnostics_and_stream_services_reach_the_service() {
    let mut config = CameraConfig::default();
    config.raw_stream.max_pending_frames = 3;
    let rig = FakeCameraRig::with_config(config);
    rig.diagnostics.reader().record_frame_acquired();
    rig.stream_counters.update(|counters| counters.frames_offered = 12);
    let running = rig.services.running().expect("запущен");
    assert_eq!(running.diagnostics_snapshot().reader.frames_acquired, 1);
    let stream = rig.services.raw_stream();
    assert_eq!(stream.counters().frames_offered, 12, "счётчики потока видны сценариям");
    assert_eq!(
        stream.pool_plan_upper_bound().pending_pairs,
        3,
        "план пула считается из конфига потока"
    );
}

#[test]
fn invalid_raw_stream_config_gives_unavailable_service_naming_the_field() {
    let mut config = CameraConfig::default();
    config.raw_stream.pool_slack = 99;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
    );
    let error = services.running().err().expect("камера недоступна");
    assert!(error.reason.contains("camera.raw_stream.pool_slack"), "{error}");
    assert!(control.calls().is_empty(), "бэкенд не создавался");
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
            raw_stream: RawStreamServices::detached(),
        },
    );
    let error = services.running().err().expect("камера недоступна");
    assert!(error.reason.contains("неверные настройки камеры"), "{error}");
    assert!(error.reason.contains("max_consecutive_attempts"), "{error}");
    assert!(control.calls().is_empty(), "бэкенд не создавался и не вызывался");
}

/// Ящик указаний AWB, переданный корнем композиции, получает указания движка: сначала умолчание при старте,
/// затем ручную точку белого после команды. Читатель (поток рендера) держит клон того же ящика.
#[test]
fn white_balance_slot_of_the_composition_root_receives_the_engine_directives() {
    use camera_contract::control::{ColorTemperature, Control, ManualControl, Tint, WhiteBalanceSetting};
    use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective, WhiteBalanceMode};

    let slot: DirectiveSlot<WhiteBalanceDirective> = DirectiveSlot::new();
    let reader = slot.clone();
    let mut config = CameraConfig::default();
    config.viewfinder_autostart.enabled = false;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera_with_white_balance(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
        slot,
    );
    let wait_for = |wanted: &dyn Fn(&WhiteBalanceDirective) -> bool| {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Some((_, directive)) = reader.latest().filter(|(_, directive)| wanted(directive)) {
                return directive;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "указание не пришло: {:?}",
                reader.latest()
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };

    let initial = wait_for(&|_| true);
    assert_eq!(
        initial.mode,
        WhiteBalanceMode::Auto,
        "при старте — автоматический режим"
    );

    let setting = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(4_000.0).expect("температура"),
        tint: Tint::NEUTRAL,
    };
    let handle = services.running().expect("запущен").handle().clone();
    handle
        .apply_controls(ManualControl {
            white_balance: Control::Manual(setting),
            ..ManualControl::AUTOMATIC
        })
        .expect("команда принята очередью");
    let manual = wait_for(&|directive| matches!(directive.mode, WhiteBalanceMode::Manual { .. }));
    assert_eq!(manual.mode, WhiteBalanceMode::Manual { setting });
}
