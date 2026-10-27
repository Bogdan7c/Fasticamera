//! Сервис камеры на настоящем потоке движка и fake-бэкенде: жизненный цикл приложения → команды → устройство.

use std::sync::mpsc;

use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_core::{AppPresence, CameraState, ClosedReason};

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_test_support::{FakeCameraRig, WAIT};
use crate::camera_wiring::{CameraBackendParts, start_camera};
use crate::raw_stream_services::RawStreamServices;

fn mode_30() -> StreamMode {
    StreamMode {
        raw_format: RawFormat::Raw10,
        readout: ReadoutAspect::FourByThree,
        frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
    }
}

/// «Сценарий» открыл основную камеру и довёл её до `Streaming`.
fn stream_wide(rig: &FakeCameraRig) {
    let handle = rig.handle();
    handle.open(CameraTarget::Role(LensRole::Wide)).expect("open");
    handle.configure(mode_30()).expect("configure");
    handle.start_streaming(CaptureIntent::Preview).expect("start");
    rig.wait_streaming();
}

fn wait_presence(rig: &FakeCameraRig, presence: AppPresence) {
    rig.handle()
        .wait_until_status(WAIT, |status| status.presence == presence)
        .unwrap_or_else(|| panic!("присутствие {presence:?} не наступило"));
}

#[test]
fn engine_starts_in_background_and_resumed_brings_it_to_foreground() {
    let rig = FakeCameraRig::new();
    assert_eq!(
        rig.handle().status().presence,
        AppPresence::Background,
        "Android не даёт камеру фоновым"
    );
    let dispatch = rig.services.app_resumed();
    assert!(matches!(dispatch, CameraDispatch::Sent(_)), "{dispatch:?}");
    wait_presence(&rig, AppPresence::Foreground);
    assert_eq!(rig.services.dispatch_counters().sent, 1);
}

#[test]
fn paused_releases_the_device_and_resumed_restores_the_stream() {
    let rig = FakeCameraRig::new();
    rig.services.app_resumed();
    stream_wide(&rig);
    assert!(rig.control.is_device_open());

    assert!(matches!(rig.services.app_paused(), CameraDispatch::Sent(_)));
    let status = rig.wait_state("closed");
    assert_eq!(
        status.state,
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );
    assert!(!rig.control.is_device_open(), "камера отпущена в фоне");

    rig.services.app_resumed();
    let restored = rig.wait_streaming();
    assert!(matches!(restored.state, CameraState::Streaming { .. }));
    assert!(
        rig.control.is_device_open(),
        "желаемое состояние восстановлено при возврате"
    );
}

#[test]
fn hold_policy_keeps_the_device_open_in_background_and_counts_it() {
    let rig = FakeCameraRig::new();
    rig.services.app_resumed();
    stream_wide(&rig);
    rig.services.hold_in_background_for_diagnostics();
    assert_eq!(
        rig.services.lifecycle_policy(),
        CameraLifecyclePolicy::HoldInBackgroundForDiagnostics
    );

    assert_eq!(rig.services.app_paused(), CameraDispatch::KeptOpenByPolicy);
    // Команда «в фон» движку не уходила: присутствие не менялось, устройство открыто.
    assert_eq!(rig.handle().status().presence, AppPresence::Foreground);
    assert!(rig.control.is_device_open());
    let counters = rig.services.dispatch_counters();
    assert_eq!(counters.kept_open_by_policy, 1);
    assert_eq!(counters.sent, 1, "ушла только команда enter_foreground");
}

#[test]
fn scenario_end_restores_policy_releases_held_camera_in_background_and_closes_the_claim() {
    let rig = FakeCameraRig::new();
    rig.services.app_resumed();
    stream_wide(&rig);
    rig.services.hold_in_background_for_diagnostics();
    assert_eq!(rig.services.app_paused(), CameraDispatch::KeptOpenByPolicy);

    let effects = rig.services.scenario_ended(AppVisibility::Background);

    assert_eq!(effects.policy, PolicyRestore::RestoredFromHold);
    assert!(
        matches!(effects.close_claimed_camera, Some(CameraDispatch::Sent(_))),
        "{effects:?}"
    );
    assert!(
        matches!(effects.release_after_hold, Some(CameraDispatch::Sent(_))),
        "{effects:?}"
    );
    assert_eq!(
        rig.services.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
    // Сначала движок закрывает камеру сценария (NotRequested), затем узнаёт, что приложение в фоне.
    let released = rig.handle().wait_until_status(WAIT, |status| {
        status.state
            == CameraState::Closed {
                reason: ClosedReason::AppInBackground,
            }
    });
    assert!(
        released.is_some(),
        "приложение в фоне, политика вернулась: камера должна быть отпущена, сейчас {:?}",
        rig.state()
    );
    assert!(!rig.control.is_device_open());
}

#[test]
fn scenario_end_in_foreground_closes_claim_without_background_release() {
    let rig = FakeCameraRig::new();
    rig.services.app_resumed();
    stream_wide(&rig);
    rig.services.hold_in_background_for_diagnostics();

    let effects = rig.services.scenario_ended(AppVisibility::Foreground);

    assert_eq!(effects.policy, PolicyRestore::RestoredFromHold);
    assert!(effects.close_claimed_camera.is_some());
    assert_eq!(
        effects.release_after_hold, None,
        "приложение на переднем плане: отпускать нечего"
    );
    let status = rig.wait_state("closed");
    assert_eq!(
        status.state,
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        }
    );
}

#[test]
fn scenario_end_without_claim_does_not_touch_a_camera_it_does_not_own() {
    let rig = FakeCameraRig::new();
    rig.services.app_resumed();
    stream_wide(&rig);
    let sent_before = rig.services.dispatch_counters().sent;

    let effects = rig.services.scenario_ended(AppVisibility::Foreground);

    assert_eq!(effects.policy, PolicyRestore::AlreadyDefault);
    assert_eq!(effects.close_claimed_camera, None);
    assert_eq!(effects.release_after_hold, None);
    assert_eq!(
        rig.services.dispatch_counters().sent,
        sent_before,
        "ни одной команды движку"
    );
    assert!(
        rig.control.is_device_open(),
        "чужая (продуктовая) камера остаётся открытой"
    );
    assert!(matches!(rig.handle().status().state, CameraState::Streaming { .. }));
}

#[test]
fn claim_is_taken_once() {
    let rig = FakeCameraRig::new();
    rig.services.claim_camera_for_scenario();
    assert!(
        rig.services
            .scenario_ended(AppVisibility::Foreground)
            .close_claimed_camera
            .is_some()
    );
    assert_eq!(
        rig.services
            .scenario_ended(AppVisibility::Foreground)
            .close_claimed_camera,
        None,
        "заявка снимается одним концом сценария"
    );
}

#[test]
fn unavailable_engine_reports_reason_and_never_sends() {
    let services = CameraServices::unavailable("нет разрешения на поток", CameraScenarioConfig::default());
    let error = services.running().err().expect("движка нет");
    assert_eq!(error.reason, "нет разрешения на поток");
    assert!(error.to_string().contains("нет разрешения на поток"));
    assert_eq!(services.app_resumed(), CameraDispatch::EngineUnavailable);
    assert_eq!(services.app_paused(), CameraDispatch::EngineUnavailable);
    services.hold_in_background_for_diagnostics();
    assert_eq!(
        services.app_paused(),
        CameraDispatch::KeptOpenByPolicy,
        "политика решает до обращения к движку"
    );
    let counters = services.dispatch_counters();
    assert_eq!(counters.engine_unavailable, 2);
    assert_eq!(counters.sent, 0);
}

#[test]
fn full_command_queue_is_counted_and_does_not_panic_or_block() {
    let (release, gate) = mpsc::channel::<()>();
    let rig_control = camera_core::fake::FakeBackendControl::new();
    let inner = rig_control.factory();
    // Поток камеры стоит в фабрике, пока тест не отпустит: очередь команд никто не читает.
    let gated: camera_core::backend::BackendFactory = Box::new(move |sink| {
        let _ = gate.recv();
        inner(sink)
    });
    let mut config = CameraConfig::default();
    config.engine.queues.command_capacity = 1;
    let diagnostics = std::sync::Arc::new(camera_android::BackendDiagnostics::new(4));
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: gated,
            diagnostics,
            raw_stream: RawStreamServices::detached(),
        },
    );

    assert!(matches!(services.app_resumed(), CameraDispatch::Sent(_)));
    assert_eq!(
        services.app_paused(),
        CameraDispatch::Rejected(CommandRejected::QueueFull)
    );
    let counters = services.dispatch_counters();
    assert_eq!((counters.sent, counters.rejected_queue_full), (1, 1));
    drop(release);
}

#[test]
fn every_dispatch_kind_lands_in_its_own_counter() {
    let cells = DispatchCounterCells::default();
    for dispatch in [
        CameraDispatch::Sent(CommandId(1)),
        CameraDispatch::Sent(CommandId(2)),
        CameraDispatch::KeptOpenByPolicy,
        CameraDispatch::EngineUnavailable,
        CameraDispatch::Rejected(CommandRejected::QueueFull),
        CameraDispatch::Rejected(CommandRejected::EngineStopped),
        CameraDispatch::Rejected(CommandRejected::EngineStopped),
    ] {
        cells.record(dispatch);
    }
    assert_eq!(
        cells.snapshot(),
        CameraDispatchCounters {
            sent: 2,
            kept_open_by_policy: 1,
            engine_unavailable: 1,
            rejected_queue_full: 1,
            rejected_engine_stopped: 2,
        }
    );
}

#[test]
fn scenario_end_disarms_an_armed_cpu_probe_and_reports_it() {
    let rig = FakeCameraRig::new();
    rig.services.raw_stream().arm_cpu_probe();
    assert!(rig.cpu_probe.is_armed(), "рубильник общий с бэкендом");

    let effects = rig.services.scenario_ended(AppVisibility::Foreground);

    assert!(effects.cpu_probe_disarmed);
    assert!(!rig.cpu_probe.is_armed(), "проба не переживает сценарий");
}

#[test]
fn scenario_end_with_a_quiet_probe_reports_nothing_to_disarm() {
    let rig = FakeCameraRig::new();
    let effects = rig.services.scenario_ended(AppVisibility::Foreground);
    assert!(!effects.cpu_probe_disarmed);
    assert!(!rig.cpu_probe.is_armed());
}
