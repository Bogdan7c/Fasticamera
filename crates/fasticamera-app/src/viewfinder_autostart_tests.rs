//! Автозапуск видоискателя: конфиг и владелец политики на настоящем движке с fake-платформой.
//! Путь через маршрутизатор (старт, фон, конец сценария) — `event_router_autostart_tests.rs`.

use camera_core::{CameraEvent, CommandOutcome};

use super::*;
use crate::camera_config::{CameraConfig, CameraConfigError};
use crate::camera_service::CameraServices;
use crate::camera_test_support::FakeCameraRig;
use crate::runtime_config::{ConfigError, RuntimeConfig};

/// Сколько событий забирать за раз: с запасом больше, чем шлёт движок за один тест.
const EVENTS_LIMIT: usize = 1_000;

/// Имена команд, которые движок обработал (по событиям `CommandHandled`), с итогами.
fn handled_commands(handle: &CameraHandle) -> Vec<(&'static str, CommandOutcome)> {
    handle
        .drain_events(EVENTS_LIMIT)
        .into_iter()
        .filter_map(|event| match event {
            CameraEvent::CommandHandled { name, outcome, .. } => Some((name, outcome)),
            _ => None,
        })
        .collect()
}

/// Ждёт, пока движок обработает `count` команд (команды асинхронны).
fn wait_for_handled(handle: &CameraHandle, count: usize) -> Vec<(&'static str, CommandOutcome)> {
    let deadline = std::time::Instant::now() + crate::camera_test_support::WAIT;
    let mut handled = Vec::new();
    while handled.len() < count && std::time::Instant::now() < deadline {
        handled.extend(handled_commands(handle));
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    handled
}

fn product_config(enabled: bool) -> CameraConfig {
    let mut config = CameraConfig::default();
    config.viewfinder_autostart.enabled = enabled;
    config
}

#[test]
fn defaults_are_the_raw_stream_mode_on_the_main_camera() {
    let config = ViewfinderAutostartConfig::default();
    assert!(config.enabled, "решение владельца в 07: видоискатель при старте");
    assert_eq!(config.role, LensRoleSetting::Wide);
    let resolved = config.resolve().expect("умолчания верны");
    assert_eq!(
        resolved.target,
        CameraTarget::Role(camera_contract::ids::LensRole::Wide)
    );
    assert_eq!(resolved.mode.raw_format, camera_contract::raw_format::RawFormat::Raw10);
    assert_eq!(
        resolved.mode.readout,
        camera_contract::stream::ReadoutAspect::FourByThree
    );
    assert_eq!(resolved.mode.frame_rate.frames_per_second(), 60);
}

#[test]
fn json_section_overrides_partially_and_rejects_typos() {
    let parse = |section: &str| {
        RuntimeConfig::from_json(&format!(
            r#"{{"build_type":"debug","log_filter":"info","camera":{{"viewfinder_autostart":{section}}}}}"#
        ))
    };
    let config = parse(r#"{"enabled":false,"mode":{"frame_rate":30}}"#).expect("частичная секция");
    let autostart = &config.camera().viewfinder_autostart;
    assert!(!autostart.enabled);
    assert_eq!(autostart.mode.frame_rate, 30);
    assert_eq!(
        autostart.mode.readout,
        ReadoutSetting::FourByThree,
        "несказанное — по умолчанию"
    );
    assert_eq!(autostart.role, LensRoleSetting::Wide);

    for typo in [r#"{"enable":true}"#, r#"{"mode":{"fps":60}}"#] {
        let error = parse(typo).expect_err(typo);
        assert!(matches!(error, ConfigError::Malformed(_)), "{typo}: {error}");
    }
}

#[test]
fn invalid_frame_rate_is_rejected_by_the_camera_section_even_when_disabled() {
    let mut config = product_config(false);
    config.viewfinder_autostart.mode.frame_rate = 0;
    let error = config.validate().expect_err("частота 0 неверна");
    assert!(matches!(error, CameraConfigError::ViewfinderAutostart(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("camera.viewfinder_autostart.mode.frame_rate"),
        "{error}"
    );
}

#[test]
fn start_once_sends_open_configure_start_exactly_once() {
    let rig = FakeCameraRig::with_config(CameraConfig::default());
    let autostart = ViewfinderAutostart::from_config(&ViewfinderAutostartConfig::default()).expect("конфиг");
    let handle = rig.handle();

    let first = autostart.start_once(Ok(&handle));
    assert!(
        matches!(first, ViewfinderAutostartOutcome::DesiredStateSent(_)),
        "{first:?}"
    );
    assert_eq!(
        autostart.start_once(Ok(&handle)),
        ViewfinderAutostartOutcome::AlreadyStarted,
        "второй старт ничего не шлёт"
    );

    let handled = wait_for_handled(&handle, 3);
    assert_eq!(
        handled,
        vec![
            ("open", CommandOutcome::Accepted),
            ("configure", CommandOutcome::Accepted),
            ("start_streaming", CommandOutcome::Accepted),
        ],
        "ровно три команды, все приняты движком"
    );
    assert!(
        !rig.control.is_device_open(),
        "движок в фоне: команды меняют только желаемое, устройство откроется по Resumed"
    );
}

#[test]
fn disabled_autostart_sends_nothing_and_restore_keeps_the_camera_closed() {
    let rig = FakeCameraRig::with_config(CameraConfig::default());
    let autostart = ViewfinderAutostart::from_config(&product_config(false).viewfinder_autostart).expect("конфиг");
    let handle = rig.handle();

    assert!(!autostart.is_enabled());
    assert_eq!(autostart.start_once(Ok(&handle)), ViewfinderAutostartOutcome::Disabled);
    assert_eq!(
        autostart.restore_after_scenario(Ok(&handle)),
        ViewfinderAutostartOutcome::Disabled
    );
    // Проверочная команда: если бы автозапуск что-то послал, его события пришли бы раньше неё.
    handle.close().expect("close");
    assert_eq!(
        wait_for_handled(&handle, 1),
        vec![("close", CommandOutcome::AlreadyInState)],
        "ни одной команды автозапуска"
    );
}

#[test]
fn unavailable_engine_is_reported_without_panic_and_counts_as_the_single_start() {
    let autostart = ViewfinderAutostart::from_config(&ViewfinderAutostartConfig::default()).expect("конфиг");
    let unavailable = || {
        Err(CameraUnavailable {
            reason: "тест: движок не поднялся".to_owned(),
        })
    };
    assert_eq!(
        autostart.start_once(unavailable()),
        ViewfinderAutostartOutcome::EngineUnavailable
    );
    assert_eq!(
        autostart.start_once(unavailable()),
        ViewfinderAutostartOutcome::AlreadyStarted
    );
    assert_eq!(
        autostart.restore_after_scenario(unavailable()),
        ViewfinderAutostartOutcome::EngineUnavailable
    );
}

#[test]
fn service_without_attached_autostart_reports_disabled() {
    let services = CameraServices::unavailable("тест", crate::camera_scenario_config::CameraScenarioConfig::default());
    assert_eq!(
        services.start_product_viewfinder(),
        ViewfinderAutostartOutcome::Disabled,
        "по умолчанию сервис автозапуска не имеет"
    );
}

#[test]
fn start_camera_with_product_config_sets_the_desired_viewfinder_once() {
    let rig = FakeCameraRig::with_product_config(product_config(true));
    let handled = wait_for_handled(&rig.handle(), 3);
    let names: Vec<_> = handled.iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["open", "configure", "start_streaming"]);
    assert_eq!(
        rig.services.start_product_viewfinder(),
        ViewfinderAutostartOutcome::AlreadyStarted,
        "wiring уже стартовал видоискатель: повтор от любого вызывающего — no-op"
    );
}

#[test]
fn full_command_queue_stops_the_chain_and_names_the_rejected_command() {
    use std::sync::mpsc;

    use crate::camera_service::BackendDiagnosticsSource;
    use crate::camera_wiring::{CameraBackendParts, start_camera};
    use crate::raw_stream_services::RawStreamServices;

    let (release, gate) = mpsc::channel::<()>();
    let control = camera_core::fake::FakeBackendControl::new();
    let inner = control.factory();
    // Поток камеры стоит в фабрике, пока тест не отпустит: очередь команд никто не читает.
    let gated: camera_core::backend::BackendFactory = Box::new(move |sink| {
        let _ = gate.recv();
        inner(sink)
    });
    let mut config = product_config(false);
    config.engine.queues.command_capacity = 1;
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: gated,
            diagnostics: std::sync::Arc::new(camera_android::BackendDiagnostics::new(4))
                as std::sync::Arc<dyn BackendDiagnosticsSource>,
            raw_stream: RawStreamServices::detached(),
        },
    );
    let handle = services.running().expect("движок запущен").handle().clone();
    let autostart = ViewfinderAutostart::from_config(&ViewfinderAutostartConfig::default()).expect("конфиг");

    assert_eq!(
        autostart.start_once(Ok(&handle)),
        ViewfinderAutostartOutcome::Rejected {
            command: AutostartCommand::Configure,
            rejected: CommandRejected::QueueFull,
        },
        "open занял очередь, configure отвергнут, start_streaming не отправлялся"
    );
    drop(release);
}
