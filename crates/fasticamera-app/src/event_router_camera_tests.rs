//! Политика жизненного цикла камеры в маршрутизаторе: события оболочки идут по настоящей очереди,
//! маршрутизатор отдаёт команды настоящему движку камеры, платформа камеры — fake.

use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_core::{CameraState, ClosedReason};

use super::*;
use crate::camera_lifecycle::CameraLifecyclePolicy;
use crate::camera_service::CameraServices;
use crate::scenario::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed};
use crate::scenario::{ScenarioConfig, ScenarioOutcome};

fn stream_wide(harness: &Harness) {
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
}

/// Чем пробный сценарий кончает работу.
#[derive(Clone, Copy)]
enum ProbeEnd {
    Succeed,
    Fail,
    Panic,
}

/// Отладочный сценарий-проба: включает удержание камеры в фоне и ждёт команды завершиться.
struct HoldProbe {
    end: Arc<Mutex<Option<ProbeEnd>>>,
}

impl ObservingScenario for HoldProbe {
    fn name(&self) -> &'static str {
        "hold-probe"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        context.camera.hold_in_background_for_diagnostics();
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(end) = self.end.lock().expect("замок").take() else {
            return ScenarioStep::Continue;
        };
        match end {
            ProbeEnd::Succeed => ScenarioStep::Finished(ScenarioOutcome::Passed {
                report: serde_json::json!({}),
            }),
            ProbeEnd::Fail => ScenarioStep::Finished(failed("дедлайн пробы")),
            ProbeEnd::Panic => std::panic::resume_unwind(Box::new("паника пробы")),
        }
    }
}

/// Сценарий, который ничего не знает о камере и сразу заканчивается.
struct NeutralProbe;

impl ObservingScenario for NeutralProbe {
    fn name(&self) -> &'static str {
        "neutral-probe"
    }

    fn start(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Finished(ScenarioOutcome::Passed {
            report: serde_json::json!({}),
        })
    }
}

/// Приложение на экране, камера «продукта» идёт, проба держит камеру в фоне, приложение ушло в фон.
fn held_camera_in_background(harness: &mut Harness) -> Arc<Mutex<Option<ProbeEnd>>> {
    harness.deliver(ShellEvent::Resumed);
    stream_wide(harness);
    let end = Arc::new(Mutex::new(None));
    harness
        .starter
        .start(Box::new(HoldProbe { end: Arc::clone(&end) }))
        .expect("старт пробы");
    harness.router.step(Duration::from_millis(2));
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::HoldInBackgroundForDiagnostics
    );
    harness.deliver(ShellEvent::Paused);
    assert!(
        harness.rig().control.is_device_open(),
        "пока проба держит камеру, фон её не отпускает"
    );
    assert_eq!(harness.camera.dispatch_counters().kept_open_by_policy, 1);
    end
}

/// После конца сценария: политика по умолчанию, камера (заявленная пробой) отпущена в фоне.
fn assert_policy_returned_and_camera_released(harness: &mut Harness) {
    harness.run_until_result();
    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
    let released = harness.rig().handle().wait_until_status(WAIT_FOR_ENGINE, |status| {
        status.state
            == CameraState::Closed {
                reason: ClosedReason::AppInBackground,
            }
    });
    assert!(released.is_some(), "камера не отпущена: {:?}", harness.rig().state());
    assert!(!harness.rig().control.is_device_open());
}

const WAIT_FOR_ENGINE: Duration = crate::camera_test_support::WAIT;

#[test]
fn paused_releases_the_camera_and_resumed_restores_the_stream_through_the_real_queue() {
    let mut harness = harness(ScenarioConfig::default());
    harness.deliver(ShellEvent::Resumed);
    stream_wide(&harness);
    assert!(harness.rig().control.is_device_open());

    harness.deliver(ShellEvent::Paused);
    let closed = harness.rig().wait_state("closed");
    assert_eq!(
        closed.state,
        CameraState::Closed {
            reason: ClosedReason::AppInBackground
        }
    );
    assert!(!harness.rig().control.is_device_open(), "в фоне устройство закрыто");

    harness.deliver(ShellEvent::Resumed);
    harness.rig().wait_streaming();
    assert!(
        harness.rig().control.is_device_open(),
        "поток восстановлен при возврате"
    );
}

#[test]
fn hold_policy_keeps_the_device_open_on_paused_and_is_restored_when_the_scenario_succeeds() {
    let mut harness = harness(ScenarioConfig::default());
    let end = held_camera_in_background(&mut harness);
    *end.lock().expect("замок") = Some(ProbeEnd::Succeed);
    assert_policy_returned_and_camera_released(&mut harness);
    assert_eq!(harness.output.single_result()["status"], "passed");
}

#[test]
fn policy_is_restored_when_the_scenario_fails_at_its_deadline() {
    let mut harness = harness(ScenarioConfig::default());
    let end = held_camera_in_background(&mut harness);
    *end.lock().expect("замок") = Some(ProbeEnd::Fail);
    assert_policy_returned_and_camera_released(&mut harness);
    assert_eq!(harness.output.single_result()["status"], "failed");
}

#[test]
fn policy_is_restored_when_the_scenario_panics() {
    let mut harness = harness(ScenarioConfig::default());
    let end = held_camera_in_background(&mut harness);
    *end.lock().expect("замок") = Some(ProbeEnd::Panic);
    assert_policy_returned_and_camera_released(&mut harness);
    assert_eq!(harness.output.single_result()["status"], "panicked");
}

#[test]
fn policy_is_restored_when_another_scenario_preempts_the_holder() {
    let mut harness = harness(ScenarioConfig::default());
    let _end = held_camera_in_background(&mut harness);
    harness.start(Scenario::Motion);
    harness.router.step(Duration::from_millis(2));

    assert_eq!(
        harness.camera.lifecycle_policy(),
        CameraLifecyclePolicy::ReleaseInBackground
    );
    let released = harness
        .rig()
        .handle()
        .wait_until_status(WAIT_FOR_ENGINE, |status| status.state.name() == "closed");
    assert!(released.is_some(), "вытесненный держатель не оставил камеру открытой");
    let results = harness.output.results.lock().expect("замок");
    assert_eq!(results[0]["scenario"], "hold-probe");
    assert!(
        results[0]["error"].as_str().expect("error").contains("вытеснен"),
        "{:?}",
        results[0]
    );
}

#[test]
fn ending_scenario_in_foreground_closes_its_claimed_camera_and_keeps_foreground_presence() {
    let mut harness = harness(ScenarioConfig::default());
    harness.deliver(ShellEvent::Resumed);
    stream_wide(&harness);
    let end = Arc::new(Mutex::new(Some(ProbeEnd::Succeed)));
    harness
        .starter
        .start(Box::new(HoldProbe { end: Arc::clone(&end) }))
        .expect("старт");
    harness.run_until_result();

    let closed = harness.rig().wait_state("closed");
    assert_eq!(
        closed.state,
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        }
    );
    assert_eq!(
        harness.rig().handle().status().presence,
        camera_core::AppPresence::Foreground
    );
}

#[test]
fn scenario_that_never_claimed_the_camera_does_not_close_the_product_camera() {
    let mut harness = harness(ScenarioConfig::default());
    harness.deliver(ShellEvent::Resumed);
    stream_wide(&harness);
    harness.starter.start(Box::new(NeutralProbe)).expect("старт");
    harness.run_until_result();
    assert_eq!(
        harness.output.single_result()["scenario"],
        "neutral-probe",
        "сценарий дошёл до итога"
    );

    assert!(harness.rig().control.is_device_open());
    assert!(matches!(harness.rig().state(), CameraState::Streaming { .. }));
}

#[test]
fn router_survives_missing_camera_engine_and_counts_skipped_commands() {
    let route = RecordingRoute::new(ReceiptPolicy::Confirm);
    let render = FakeRenderStatus::live(presenting_status(), ValidationState::Enabled);
    let diagnostics = FakeGpuDiagnostics::new(Vec::new(), None);
    let camera = CameraServices::unavailable(
        "камера не поднялась",
        crate::camera_scenario_config::CameraScenarioConfig::default(),
    );
    let mut harness = build_harness_with_camera(
        ScenarioConfig::default(),
        Box::new(route),
        render,
        diagnostics,
        RenderLoopConfig::default(),
        camera,
    );

    harness.deliver(ShellEvent::Resumed);
    harness.deliver(ShellEvent::Paused);

    let counters = harness.camera.dispatch_counters();
    assert_eq!(counters.engine_unavailable, 2);
    assert_eq!(counters.sent, 0);
}
