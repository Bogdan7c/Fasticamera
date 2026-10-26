//! Функциональные тесты `camera-evict`: вытеснение камеры системной камерой и самовосстановление движка.
//! Только отладочная сборка (`debug-scenarios`). Настоящий маршрутизатор и движок, платформа — fake.

use std::time::Instant;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::control::CaptureIntent;
use camera_contract::fixtures::camera_id;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_core::backend::{BackendEvent, CameraAvailability};

use super::super::*;
use super::*;
use crate::camera_lifecycle::CameraLifecyclePolicy;
use crate::camera_scenario_config::EvictScenarioConfig;

fn evict_rig(streaming_ms: u64, eviction_ms: u64, recovery_ms: u64) -> FakeCameraRig {
    FakeCameraRig::with_scenarios(CameraScenarioConfig {
        evict: EvictScenarioConfig {
            streaming_timeout_ms: streaming_ms,
            eviction_timeout_ms: eviction_ms,
            recovery_timeout_ms: recovery_ms,
            ..EvictScenarioConfig::default()
        },
        ..CameraScenarioConfig::default()
    })
}

/// Крутит маршрутизатор, пока условие не выполнится (или не выйдет срок).
fn step_until(harness: &mut Harness, done: impl Fn(&Harness) -> bool) {
    let deadline = Instant::now() + StdDuration::from_secs(5);
    while !done(harness) && Instant::now() < deadline {
        harness.router.step(StdDuration::from_millis(2));
    }
    assert!(done(harness), "условие не наступило за 5 с");
}

fn start_evict(harness: &mut Harness) {
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("camera-evict").expect("сценарий зарегистрирован"));
    step_until(harness, |harness| {
        harness.output.stage_names().contains(&"streaming".to_owned())
    });
}

#[test]
fn eviction_by_the_system_camera_is_survived_and_the_stream_recovers() {
    let mut harness = harness_for_rig(evict_rig(3_000, 5_000, 5_000));
    start_evict(&mut harness);
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::HoldInBackgroundForDiagnostics,
        "на время работы сценарий держит камеру"
    );

    // Скрипт запустил системную камеру: приложение ушло в фон, система отняла камеру.
    harness.deliver(ShellEvent::Paused);
    assert!(
        harness.rig().control.is_device_open(),
        "в фоне камера не отпущена: иначе нечему быть вытесненным"
    );
    harness.rig().control.disconnect_current_device();
    step_until(&mut harness, |harness| {
        harness.rig().handle().status().last_unavailable == Some(CameraUnavailableReason::Evicted)
    });
    // Системная камера закрылась, приложение вернулось.
    harness.deliver(ShellEvent::Resumed);
    harness.rig().control.send_event(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    harness.run_until_result();
    let result = harness.output.single_result();

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["unavailable"]["reason"], "evicted");
    assert_eq!(report["camera"], "2");
    assert_eq!(report["final_state"], "streaming");
    assert_eq!(report["counters"]["recoveries"], 1);
    assert_fits_logcat(&harness, &result);
    assert!(report["recovered"]["attempts"].as_u64().expect("attempts") >= 1);
    assert!(report["recovered"]["recovery_ms"].is_u64());
    let kinds: Vec<&str> = report["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["kind"].as_str().expect("kind"))
        .collect();
    let evicted_at = kinds
        .iter()
        .position(|kind| *kind == "unavailable")
        .expect("недоступность в журнале");
    let recovered_at = kinds
        .iter()
        .position(|kind| *kind == "recovered")
        .expect("восстановление в журнале");
    assert!(evicted_at < recovered_at, "{kinds:?}");
    // Конец сценария: политика вернулась, камера закрыта.
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
    harness.rig().wait_state("closed");
}

#[test]
fn streaming_of_a_previous_session_is_not_taken_for_the_scenario_stream() {
    let rig = evict_rig(3_000, 5_000, 5_000);
    let mut harness = harness_for_rig(rig);
    harness.deliver(ShellEvent::Resumed);
    let handle = harness.rig().handle();
    handle.open(CameraTarget::Role(LensRole::Wide)).expect("open");
    handle
        .configure(StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::FourByThree,
            frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
        })
        .expect("configure");
    handle.start_streaming(CaptureIntent::Preview).expect("start");
    harness.rig().wait_streaming();
    // Закрытие старого устройства занимает 150 мс: всё это время снимок показывает старый Streaming.
    harness
        .rig()
        .close_device_delay_ms
        .store(150, std::sync::atomic::Ordering::SeqCst);

    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("camera-evict").expect("сценарий зарегистрирован"));
    for _ in 0..30 {
        harness.router.step(StdDuration::from_millis(2));
    }

    assert!(
        !harness.output.stage_names().contains(&"streaming".to_owned()),
        "стадию streaming нельзя писать по потоку прошлой сессии: скрипт запустил бы системную камеру раньше времени"
    );
    harness
        .rig()
        .close_device_delay_ms
        .store(0, std::sync::atomic::Ordering::SeqCst);
    step_until(&mut harness, |harness| {
        harness.output.stage_names().contains(&"streaming".to_owned())
    });
}

#[test]
fn no_eviction_within_the_deadline_fails_and_still_restores_policy_and_closes_camera() {
    let mut harness = harness_for_rig(evict_rig(3_000, 150, 5_000));
    start_evict(&mut harness);
    harness.run_until_result();
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(result["report"]["failure"]["kind"], "never_evicted");
    assert!(
        result["error"].as_str().expect("error").contains("вытеснения не было"),
        "{result}"
    );
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
    harness.rig().wait_state("closed");
}

#[test]
fn evicted_but_never_recovered_fails_with_the_reason_that_arrived() {
    let mut harness = harness_for_rig(evict_rig(3_000, 5_000, 150));
    start_evict(&mut harness);
    harness.deliver(ShellEvent::Paused);
    harness.rig().control.disconnect_current_device();
    // Приложение не возвращается, камера не освобождается.
    harness.run_until_result();
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(result["report"]["unavailable"]["reason"], "evicted");
    assert_eq!(result["report"]["failure"]["kind"], "never_recovered");
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
}

#[test]
fn stream_that_never_starts_fails_without_waiting_for_the_system_camera() {
    let mut harness = harness_for_rig(evict_rig(150, 5_000, 5_000));
    // Resumed не приходит: движок в фоне и камеру не откроет.
    harness.start(Scenario::parse("camera-evict").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    assert_eq!(result["report"]["failure"]["kind"], "never_streaming");
    assert!(
        !harness.output.stage_names().contains(&"streaming".to_owned()),
        "скрипт не получил стадии streaming"
    );
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
}

#[test]
fn evict_scenario_on_an_unavailable_engine_fails_with_its_reason() {
    let mut harness = harness_for_services(CameraServices::unavailable(
        "нет камеры",
        CameraScenarioConfig::default(),
    ));
    harness.start(Scenario::parse("camera-evict").expect("сценарий зарегистрирован"));
    harness.run_until_result();
    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"].as_str().expect("error").contains("нет камеры"),
        "{result}"
    );
}

#[test]
fn evict_scenario_is_registered_in_debug_builds() {
    assert_eq!(
        Scenario::parse("camera-evict").expect("есть в debug").name(),
        "camera-evict"
    );
}
