//! Автозапуск видоискателя через настоящий маршрутизатор: события оболочки идут по очереди,
//! движок камеры настоящий, платформа камеры — fake. Проверяется продуктовое поведение: старт
//! приложения показывает поток основной камеры 4:3@60 RAW10, фон/передний план не повторяют
//! команд автозапуска, конец сценария камеры возвращает видоискатель, а прочие сценарии его не трогают.

use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraTarget, LensFacing, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_contract::zoom::{LensIntent, ZoomRatio};
use camera_core::{CameraEvent, CameraState, CameraStatus, ClosedReason};

use super::*;
use crate::camera_config::CameraConfig;
use crate::camera_service::CameraServices;
use crate::camera_test_support::WAIT;
use crate::scenario::ScenarioOutcome;
use crate::scenario::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep};
use crate::viewfinder_autostart::{ViewfinderAutostart, ViewfinderAutostartConfig, ViewfinderAutostartOutcome};

/// Сколько событий движка забирать за раз.
const EVENTS_LIMIT: usize = 1_000;
/// Камера роли `wide` в каталоге OnePlus 11.
const MAIN_CAMERA_ID: &str = "2";

fn product_rig(enabled: bool) -> FakeCameraRig {
    let mut config = CameraConfig::default();
    config.viewfinder_autostart.enabled = enabled;
    FakeCameraRig::with_product_config(config)
}

/// Сколько раз движок обработал команду с этим именем (по уже пришедшим событиям).
fn count_handled(events: &[CameraEvent], command: &str) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CameraEvent::CommandHandled { name, .. } if *name == command))
        .count()
}

/// Поток идёт на основной камере в режиме видоискателя продукта (4:3@60 RAW10).
fn assert_product_viewfinder(status: &CameraStatus) {
    let CameraState::Streaming { stream } = &status.state else {
        panic!("ожидался поток, сейчас {}", status.state.name());
    };
    assert_eq!(stream.stream.camera.as_str(), MAIN_CAMERA_ID);
    assert_eq!(stream.stream.readout, ReadoutAspect::FourByThree);
    assert_eq!(stream.stream.raw_format, RawFormat::Raw10);
    assert_eq!(stream.stream.frame_rate.frames_per_second(), 60);
}

/// Ждёт потока на основной камере в режиме видоискателя.
fn wait_product_viewfinder(rig: &FakeCameraRig) -> CameraStatus {
    let status = rig
        .handle()
        .wait_until_status(WAIT, |status| match &status.state {
            CameraState::Streaming { stream } => {
                stream.stream.camera.as_str() == MAIN_CAMERA_ID
                    && stream.stream.readout == ReadoutAspect::FourByThree
                    && stream.stream.frame_rate.frames_per_second() == 60
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("видоискатель не вернулся: {:?}", rig.state()));
    assert_product_viewfinder(&status);
    status
}

/// Сценарий камеры: заявляет права и переключает поток на камеру `role` в режиме 16:9@30, затем заканчивается.
struct RoleProbe {
    role: LensRole,
    finish_now: Arc<Mutex<bool>>,
}

impl ObservingScenario for RoleProbe {
    fn name(&self) -> &'static str {
        "ultra-wide-probe"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        context.camera.claim_camera_for_scenario();
        let running = context.camera.running().expect("движок");
        let handle = running.handle();
        handle.close().expect("close");
        handle.open(CameraTarget::Role(self.role)).expect("open");
        handle
            .configure(StreamMode {
                raw_format: RawFormat::Raw10,
                readout: ReadoutAspect::SixteenByNine,
                frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
            })
            .expect("configure");
        handle.start_streaming(CaptureIntent::Preview).expect("start");
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if *self.finish_now.lock().expect("замок") {
            ScenarioStep::Finished(ScenarioOutcome::Passed {
                report: serde_json::json!({}),
            })
        } else {
            ScenarioStep::Continue
        }
    }
}

/// Сценарий, который камеры не касается и сразу заканчивается.
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

#[test]
fn app_start_shows_the_main_camera_viewfinder_after_resumed() {
    let mut harness = harness_for_rig(product_rig(true));
    assert!(
        !harness.rig().control.is_device_open(),
        "до Resumed приложение в фоне: Android камеру не даст, движок её не открывает"
    );

    harness.deliver(ShellEvent::Resumed);
    wait_product_viewfinder(harness.rig());

    let events = harness.rig().handle().drain_events(EVENTS_LIMIT);
    for command in ["open", "configure", "start_streaming"] {
        assert_eq!(count_handled(&events, command), 1, "{command} — ровно один раз");
    }
    assert_eq!(
        harness.rig().control.repeating_request().map(|(intent, _)| intent),
        Some(CaptureIntent::Preview),
        "видоискатель — запрос Preview"
    );
}

#[test]
fn paused_and_resumed_do_not_reissue_autostart_commands_the_engine_restores_the_stream() {
    let mut harness = harness_for_rig(product_rig(true));
    harness.deliver(ShellEvent::Resumed);
    wait_product_viewfinder(harness.rig());
    let before = harness.rig().handle().drain_events(EVENTS_LIMIT);
    assert_eq!(count_handled(&before, "open"), 1);

    for _cycle in 0..2 {
        harness.deliver(ShellEvent::Paused);
        let closed = harness.rig().wait_state("closed");
        assert_eq!(
            closed.state,
            CameraState::Closed {
                reason: ClosedReason::AppInBackground
            }
        );
        harness.deliver(ShellEvent::Resumed);
        wait_product_viewfinder(harness.rig());
    }

    let after = harness.rig().handle().drain_events(EVENTS_LIMIT);
    for command in ["open", "configure", "start_streaming", "close"] {
        assert_eq!(count_handled(&after, command), 0, "{command} не повторялась");
    }
    assert_eq!(count_handled(&after, "enter_background"), 2);
    assert_eq!(count_handled(&after, "enter_foreground"), 2);
}

#[test]
fn camera_scenario_end_restores_the_product_viewfinder() {
    let mut harness = harness_for_rig(product_rig(true));
    harness.deliver(ShellEvent::Resumed);
    wait_product_viewfinder(harness.rig());

    let finish_now = Arc::new(Mutex::new(false));
    harness
        .starter
        .start(Box::new(RoleProbe {
            role: LensRole::UltraWide,
            finish_now: Arc::clone(&finish_now),
        }))
        .expect("старт пробы");
    harness.router.step(Duration::from_millis(2));
    let probe_stream = harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| {
            matches!(&status.state, CameraState::Streaming { stream } if stream.stream.readout == ReadoutAspect::SixteenByNine)
        })
        .expect("сценарий открыл свой поток");
    let CameraState::Streaming { stream } = probe_stream.state else {
        unreachable!("ждали именно поток");
    };
    assert_ne!(stream.stream.camera.as_str(), MAIN_CAMERA_ID, "у сценария — ширик");

    *finish_now.lock().expect("замок") = true;
    harness.run_until_result();
    assert_eq!(harness.output.single_result()["status"], "passed");

    wait_product_viewfinder(harness.rig());
    assert_eq!(harness.camera.dispatch_counters().sent, 2, "Resumed и close сценария");
}

/// Камера роли `ultra_wide` в каталоге OnePlus 11.
const ULTRA_WIDE_CAMERA_ID: &str = "3";

/// Поток идёт на камере с этим ID.
fn wait_streaming_on(rig: &FakeCameraRig, camera: &str) -> CameraStatus {
    rig.handle()
        .wait_until_status(WAIT, |status| {
            matches!(&status.state, CameraState::Streaming { stream } if stream.stream.camera.as_str() == camera)
        })
        .unwrap_or_else(|| panic!("поток на камере {camera} не наступил: {:?}", rig.state()))
}

#[test]
fn scenario_end_brings_back_the_lens_the_user_had_chosen_and_not_the_default_one() {
    let mut harness = harness_for_rig(product_rig(true));
    harness.deliver(ShellEvent::Resumed);
    wait_product_viewfinder(harness.rig());

    // Пользователь выбрал 0,6× тем же путём, что экран: через ящик намерений объектива.
    harness.camera.lens_intents().publish(LensIntent {
        facing: LensFacing::Back,
        ratio: ZoomRatio::new(0.6).expect("коэффициент"),
        gesture: camera_contract::zoom::ZoomGesture::Discrete,
        standby: camera_contract::zoom::StandbyDemand::NotNeeded,
    });
    let chosen = wait_streaming_on(harness.rig(), ULTRA_WIDE_CAMERA_ID);
    assert_eq!(
        chosen.lens.active.as_ref().map(|active| active.role),
        Some(LensRole::UltraWide)
    );

    // Сценарий уводит поток на основную камеру и заканчивается.
    let finish_now = Arc::new(Mutex::new(false));
    harness
        .starter
        .start(Box::new(RoleProbe {
            role: LensRole::Wide,
            finish_now: Arc::clone(&finish_now),
        }))
        .expect("старт пробы");
    harness.router.step(Duration::from_millis(2));
    wait_streaming_on(harness.rig(), MAIN_CAMERA_ID);
    *finish_now.lock().expect("замок") = true;
    harness.run_until_result();
    assert_eq!(harness.output.single_result()["status"], "passed");

    // Видоискатель вернулся на ширик с режимом продукта, а не на основную камеру по умолчанию.
    let restored = wait_streaming_on(harness.rig(), ULTRA_WIDE_CAMERA_ID);
    let CameraState::Streaming { stream } = &restored.state else {
        unreachable!("ждали поток");
    };
    assert_eq!(stream.stream.readout, ReadoutAspect::FourByThree);
    assert_eq!(stream.stream.frame_rate.frames_per_second(), 60);
    assert_eq!(
        restored.lens.active.as_ref().map(|active| active.role),
        Some(LensRole::UltraWide)
    );
}

#[test]
fn scenario_end_without_a_user_lens_choice_returns_the_default_main_camera() {
    let mut harness = harness_for_rig(product_rig(true));
    harness.deliver(ShellEvent::Resumed);
    wait_product_viewfinder(harness.rig());

    let finish_now = Arc::new(Mutex::new(true));
    harness
        .starter
        .start(Box::new(RoleProbe {
            role: LensRole::UltraWide,
            finish_now,
        }))
        .expect("старт пробы");
    harness.run_until_result();

    wait_product_viewfinder(harness.rig());
}

#[test]
fn scenario_that_never_claimed_the_camera_leaves_the_viewfinder_session_untouched() {
    let mut harness = harness_for_rig(product_rig(true));
    harness.deliver(ShellEvent::Resumed);
    let before = wait_product_viewfinder(harness.rig());
    let session_before = harness.rig().control.current_session_generation();
    let _ = harness.rig().handle().drain_events(EVENTS_LIMIT);

    harness.starter.start(Box::new(NeutralProbe)).expect("старт");
    harness.run_until_result();
    assert_eq!(harness.output.single_result()["scenario"], "neutral-probe");

    let after = harness.rig().handle().status();
    assert_eq!(after.state, before.state, "поток тот же");
    assert_eq!(
        harness.rig().control.current_session_generation(),
        session_before,
        "сессия не пересоздавалась"
    );
    let events = harness.rig().handle().drain_events(EVENTS_LIMIT);
    for command in ["close", "open", "configure", "start_streaming"] {
        assert_eq!(count_handled(&events, command), 0, "{command} не отправлялась");
    }
}

#[test]
fn disabled_autostart_opens_nothing_and_scenario_end_leaves_the_camera_closed() {
    let mut harness = harness_for_rig(product_rig(false));
    harness.deliver(ShellEvent::Resumed);
    let foreground = harness
        .rig()
        .handle()
        .wait_until_status(WAIT, |status| status.presence == camera_core::AppPresence::Foreground)
        .expect("движок на переднем плане");
    assert_eq!(
        foreground.state,
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        }
    );
    assert!(
        !harness.rig().control.is_device_open(),
        "автозапуск выключен — камера закрыта"
    );

    let finish_now = Arc::new(Mutex::new(true));
    harness
        .starter
        .start(Box::new(RoleProbe {
            role: LensRole::UltraWide,
            finish_now,
        }))
        .expect("старт пробы");
    harness.run_until_result();
    let closed = harness.rig().wait_state("closed");
    assert_eq!(
        closed.state,
        CameraState::Closed {
            reason: ClosedReason::NotRequested
        },
        "как до 07: камера сценария закрыта, видоискатель не возвращается"
    );
}

#[test]
fn unavailable_camera_engine_with_autostart_does_not_panic_the_router() {
    let camera = CameraServices::unavailable("тест: движок не поднялся", Default::default()).with_viewfinder_autostart(
        ViewfinderAutostart::from_config(&ViewfinderAutostartConfig::default()).expect("конфиг"),
    );
    assert_eq!(
        camera.start_product_viewfinder(),
        ViewfinderAutostartOutcome::EngineUnavailable
    );
    let mut harness = harness_for_services(camera);
    harness.deliver(ShellEvent::Resumed);
    harness.deliver(ShellEvent::Paused);
    harness.starter.start(Box::new(NeutralProbe)).expect("старт");
    harness.run_until_result();

    assert_eq!(harness.output.single_result()["status"], "passed");
    assert_eq!(harness.camera.dispatch_counters().engine_unavailable, 2);
}
