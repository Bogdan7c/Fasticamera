//! Функциональные тесты через настоящий поток: handle → поток `fasticamera-camera` → fake →
//! события → снимок. Ожидания ограничены дедлайном, ничто не висит вечно.

use std::sync::mpsc;
use std::time::Duration;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::CameraDescriptor;
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::CaptureIntent;
use camera_contract::fixtures::camera_id;
use camera_contract::ids::{CameraId, CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ConfiguredStream, ReadoutAspect, ResolvedStream, StreamMode, TargetFrameRate};
use camera_contract::three_a::{BurstId, ExposureBracket, PointRequestId};

use super::*;
use crate::backend::{
    BackendEvent, BackendFailure, CameraAvailability, CameraBackend, ConfigureFailure, DeviceGeneration, OpenFailure,
    RequestFailure, SessionGeneration,
};
use crate::command::{CommandOutcome, CommandRejected, InvalidStateReason};
use crate::event::CameraEvent;
use crate::fake::{BackendMethod, FakeBackendControl};
use crate::request_controls::{AfTrigger, RequestControls};
use crate::state::{CameraError, CameraState, FatalCause};
use crate::status::CameraStatus;

const WAIT: Duration = Duration::from_secs(10);

fn mode_60() -> StreamMode {
    StreamMode {
        raw_format: RawFormat::Raw10,
        readout: ReadoutAspect::SixteenByNine,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
    }
}

fn wide() -> CameraTarget {
    CameraTarget::Role(LensRole::Wide)
}

fn spawn_with(config: CameraEngineConfig, fake: &Arc<FakeBackendControl>) -> (CameraRuntime, CameraHandle) {
    CameraRuntime::spawn(config, AppPresence::Foreground, fake.factory()).expect("поток камеры запускается")
}

fn spawn_default(fake: &Arc<FakeBackendControl>) -> (CameraRuntime, CameraHandle) {
    spawn_with(CameraEngineConfig::default(), fake)
}

fn wait_state(handle: &CameraHandle, name: &'static str) -> CameraStatus {
    handle
        .wait_until_status(WAIT, |status| status.state.name() == name)
        .unwrap_or_else(|| panic!("состояние {name} не наступило, сейчас {}", handle.status().state.name()))
}

fn start_stream(handle: &CameraHandle) {
    handle.open(wide()).expect("команда принята");
    handle.configure(mode_60()).expect("команда принята");
    handle.start_streaming(CaptureIntent::Preview).expect("команда принята");
    wait_state(handle, "streaming");
}

fn outcome_of(events: &[CameraEvent], id: CommandId) -> Option<CommandOutcome> {
    events.iter().find_map(|event| event.command_outcome(id)).cloned()
}

fn result_meta(frame: i64) -> CaptureResultMeta {
    CaptureResultMeta {
        frame_number: FrameNumber(frame),
        sensor_timestamp: Some(SensorTimestamp::from_nanos(frame * 16_666_667)),
        exposure_time: None,
        sensitivity_iso: None,
        reported_frame_duration: None,
        rolling_shutter_skew: None,
        three_a: camera_contract::three_a::ThreeAResultState::unreported(
            camera_contract::three_a::RequestOrigin::Repeating,
        ),
    }
}

/// Бэкенд, который застревает в `load_catalog`, пока тест не отпустит ворота: «застрявший» поток.
struct GatedBackend {
    inner: Box<dyn CameraBackend>,
    /// Сообщает тесту, что поток дошёл до `load_catalog` и теперь стоит на воротах.
    entered: mpsc::Sender<()>,
    gate: mpsc::Receiver<()>,
}

impl CameraBackend for GatedBackend {
    fn load_catalog(&mut self) -> Result<Vec<CameraDescriptor>, BackendFailure> {
        let _ = self.entered.send(());
        // Ждём сигнал (или закрытие канала: тест кончился).
        let _ = self.gate.recv();
        self.inner.load_catalog()
    }
    fn open_device(&mut self, camera: &CameraId, generation: DeviceGeneration) -> Result<(), OpenFailure> {
        self.inner.open_device(camera, generation)
    }
    fn configure_session(
        &mut self,
        stream: &ResolvedStream,
        generation: SessionGeneration,
    ) -> Result<ConfiguredStream, ConfigureFailure> {
        self.inner.configure_session(stream, generation)
    }
    fn set_repeating_request(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
    ) -> Result<(), RequestFailure> {
        self.inner.set_repeating_request(intent, controls)
    }
    fn submit_focus_trigger(
        &mut self,
        intent: CaptureIntent,
        controls: &RequestControls,
        trigger: AfTrigger,
        request: PointRequestId,
    ) -> Result<(), RequestFailure> {
        self.inner.submit_focus_trigger(intent, controls, trigger, request)
    }
    fn submit_bracket(
        &mut self,
        intent: CaptureIntent,
        base: &RequestControls,
        bracket: &ExposureBracket,
        burst: BurstId,
    ) -> Result<(), RequestFailure> {
        self.inner.submit_bracket(intent, base, bracket, burst)
    }
    fn stop_repeating(&mut self) -> Result<(), RequestFailure> {
        self.inner.stop_repeating()
    }
    fn close_session(&mut self) {
        self.inner.close_session();
    }
    fn close_device(&mut self) {
        self.inner.close_device();
    }
}

/// Фабрика застрявшего бэкенда, «ключ» от ворот и сигнал «поток дошёл до ворот».
fn gated_factory(fake: &Arc<FakeBackendControl>) -> (BackendFactory, mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (release, gate) = mpsc::channel();
    let (entered, entered_signal) = mpsc::channel();
    let inner = fake.factory();
    let factory: BackendFactory = Box::new(move |sink| {
        let backend = inner(sink)?;
        Ok(Box::new(GatedBackend {
            inner: backend,
            entered,
            gate,
        }) as Box<dyn CameraBackend>)
    });
    (factory, release, entered_signal)
}

#[test]
fn camera_streams_end_to_end_through_the_real_thread() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    let open = handle.open(wide()).expect("принята");
    let configure = handle.configure(mode_60()).expect("принята");
    let start = handle.start_streaming(CaptureIntent::Preview).expect("принята");
    let status = wait_state(&handle, "streaming");
    assert_eq!(status.catalog.cameras().len(), 4, "каталог прочитан на потоке камеры");
    assert_eq!(status.configured.map(|c| c.stream.camera), Some(camera_id("2")));

    // Кадры идут от fake через ящик в темп.
    fake.complete_frames(120, Duration::from_nanos(16_666_667));
    let status = handle
        .wait_until_status(WAIT, |status| status.stream.results_completed == 120)
        .expect("120 результатов дошли");
    let fps = status.stream.measured_frames_per_second.expect("есть интервалы");
    assert!((fps - 60.0).abs() < 0.01, "{fps}");
    assert_eq!(status.counters.dropped_capture_events, 0);

    let events = handle.drain_events(1000);
    for id in [open, configure, start] {
        assert_eq!(outcome_of(&events, id), Some(CommandOutcome::Accepted));
    }
    let states: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::StateChanged { state } => Some(state.name()),
            _ => None,
        })
        .collect();
    assert_eq!(states.last(), Some(&"streaming"));
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn eviction_and_recovery_work_through_the_real_thread() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    start_stream(&handle);
    handle.drain_events(1000);

    fake.disconnect_current_device();
    let status = wait_state(&handle, "error");
    assert_eq!(status.last_unavailable, Some(CameraUnavailableReason::Evicted));
    assert!(!fake.is_device_open(), "устройство закрыто на бэкенде");

    // Камера снова свободна — движок сам поднимает тот же поток.
    fake.send_event(BackendEvent::CameraAvailability {
        camera: camera_id("2"),
        availability: CameraAvailability::Available,
    });
    let status = wait_state(&handle, "streaming");
    assert_eq!((status.counters.recovery_attempts, status.counters.recoveries), (1, 1));
    let events = handle.drain_events(1000);
    assert!(events.contains(&CameraEvent::Recovered {
        reason: CameraUnavailableReason::Evicted,
        attempts: 1
    }));
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn a_flood_of_results_under_a_stuck_engine_drops_oldest_but_never_loses_device_disconnect() {
    let fake = FakeBackendControl::new();
    let (factory, release, entered) = gated_factory(&fake);
    let (runtime, handle) = CameraRuntime::spawn(CameraEngineConfig::default(), AppPresence::Foreground, factory)
        .expect("поток запускается");
    entered.recv_timeout(WAIT).expect("поток дошёл до ворот");
    let capacity = CameraEngineConfig::default().queues.capture_event_capacity as u64;

    // Поток камеры «застрял» в load_catalog; в ящик тем временем сыплются 1000 результатов,
    // и среди них приходит отключение устройства.
    for frame in 0..1000 {
        fake.send_event(BackendEvent::CaptureCompleted {
            session: SessionGeneration(1),
            result: result_meta(frame),
        });
        if frame == 500 {
            fake.send_event(BackendEvent::DeviceDisconnected {
                device: DeviceGeneration(7),
            });
        }
    }
    assert_eq!(
        handle.status().counters.dropped_capture_events,
        1000 - capacity,
        "вытеснены старые, счётчик виден сразу, пока движок стоит"
    );

    release.send(()).expect("ворота отпущены");
    // После разблокировки движок обработал и оставшиеся кадры, и отключение; сессии нет,
    // поэтому всё это stale: capacity кадров + 1 отключение. Если бы отключение потерялось,
    // счётчик был бы capacity.
    let status = handle
        .wait_until_status(WAIT, |status| status.counters.stale_backend_events == capacity + 1)
        .expect("отключение не потеряно среди кадров");
    assert_eq!(status.counters.dropped_capture_events, 1000 - capacity);
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn one_shot_results_survive_a_flood_under_a_stuck_engine() {
    let fake = FakeBackendControl::new();
    let (factory, release, entered) = gated_factory(&fake);
    let (runtime, handle) = CameraRuntime::spawn(CameraEngineConfig::default(), AppPresence::Foreground, factory)
        .expect("поток запускается");
    entered.recv_timeout(WAIT).expect("поток дошёл до ворот");
    let capacity = CameraEngineConfig::default().queues.capture_event_capacity as u64;

    // 1000 результатов потока и три разовых (триггер тапа, два кадра серии) под застрявшим движком.
    for frame in 0..1000 {
        fake.send_event(BackendEvent::CaptureCompleted {
            session: SessionGeneration(1),
            result: result_meta(frame),
        });
        if frame % 300 == 0 {
            let mut one_shot = result_meta(10_000 + frame);
            one_shot.three_a.origin = camera_contract::three_a::RequestOrigin::Burst {
                burst: camera_contract::three_a::BurstId(1),
                index: (frame / 300) as u32,
            };
            fake.send_event(BackendEvent::CaptureCompleted {
                session: SessionGeneration(1),
                result: one_shot,
            });
        }
    }
    assert_eq!(handle.status().counters.dropped_capture_events, 1000 - capacity);
    release.send(()).expect("ворота отпущены");
    // Сессии нет, поэтому всё stale: оставшиеся кадры потока + все четыре разовых результата.
    handle
        .wait_until_status(WAIT, |status| status.counters.stale_backend_events == capacity + 4)
        .expect("разовые результаты не потеряны среди кадров");
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn waker_calls_from_other_threads_coalesce_into_one_refresh_of_the_exposure_controller() {
    use crate::three_a_vendor::VendorFocus;
    use crate::three_a_vendor::scripted::ScriptedExposureController;
    use crate::three_a_wiring::ThreeAWiring;

    let fake = FakeBackendControl::new();
    let (factory, release, entered) = gated_factory(&fake);
    let config = CameraEngineConfig::default();
    let (exposure, script) = ScriptedExposureController::new(crate::request_controls::ExposurePlan::VendorAuto {
        compensation_steps: 0,
        lock: camera_contract::three_a::AeAfLock::Unlocked,
        region: None,
    });
    let wiring = ThreeAWiring::vendor(&config.three_a).with_controllers(
        Box::new(exposure),
        Box::new(VendorFocus::new(config.three_a.focus_marker_grace_frames)),
    );
    let (runtime, handle) =
        CameraRuntime::spawn_with_wiring(config, AppPresence::Foreground, factory, wiring).expect("поток запускается");
    entered.recv_timeout(WAIT).expect("поток дошёл до ворот");

    // Поток камеры стоит; 200 пробуждений с двух потоков приходят в ящик.
    let wakers: Vec<_> = (0..2)
        .map(|_| {
            let waker = handle.three_a_waker();
            std::thread::spawn(move || (0..100).for_each(|_| waker.wake()))
        })
        .collect();
    for waker in wakers {
        waker.join().expect("поток пробуждений дожил");
    }
    release.send(()).expect("ворота отпущены");
    // Команда после пробуждений: к её ответу пробуждение уже обработано (оно в ящике раньше команд).
    handle.close().expect("команда принята");
    handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled == 1)
        .expect("команда обработана");
    assert_eq!(script.refresh_calls(), 1, "200 пробуждений — один вызов refresh()");
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn command_queue_overflow_is_reported_to_the_sender_and_accepted_commands_still_run() {
    let fake = FakeBackendControl::new();
    let (factory, release, _entered) = gated_factory(&fake);
    let mut config = CameraEngineConfig::default();
    config.queues.command_capacity = 3;
    let (runtime, handle) = CameraRuntime::spawn(config, AppPresence::Foreground, factory).expect("поток запускается");

    let accepted: Vec<CommandId> = (0..3).map(|_| handle.close().expect("место есть")).collect();
    assert_eq!(handle.close(), Err(CommandRejected::QueueFull));
    assert_eq!(handle.stop_streaming(), Err(CommandRejected::QueueFull));

    release.send(()).expect("ворота отпущены");
    handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled == 3)
        .expect("три принятые команды исполнены");
    let events = handle.drain_events(100);
    for id in accepted {
        assert!(outcome_of(&events, id).is_some(), "команда {id:?} получила итог");
    }
    // Очередь освободилась.
    assert!(handle.close().is_ok());
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn panic_in_the_backend_marks_the_engine_panicked_and_the_thread_keeps_answering() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    wait_state(&handle, "closed");
    fake.push_panic_on(BackendMethod::OpenDevice);

    let open = handle.open(wide()).expect("принята");
    let status = wait_state(&handle, "error");
    assert!(matches!(
        status.state,
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::EnginePanicked { ref message }
            }
        } if message.contains("запрограммированная паника")
    ));
    let after = handle.open(wide()).expect("очередь жива");
    let close = handle.close().expect("очередь жива");
    handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled == 3)
        .expect("поток отвечает после паники");

    let events = handle.drain_events(100);
    assert_eq!(
        outcome_of(&events, open),
        Some(CommandOutcome::Accepted),
        "итог опубликован до паники"
    );
    let failed = CommandOutcome::InvalidState(InvalidStateReason::EngineFailed);
    assert_eq!(outcome_of(&events, after), Some(failed.clone()));
    assert_eq!(outcome_of(&events, close), Some(failed));
    assert!(!fake.is_device_open());
    assert_eq!(
        runtime.shutdown(WAIT),
        ShutdownReport::Stopped,
        "поток всё ещё корректно останавливается"
    );
}

#[test]
fn backend_factory_failure_is_fatal_but_the_thread_answers_commands() {
    let (runtime, handle) = CameraRuntime::spawn(
        CameraEngineConfig::default(),
        AppPresence::Foreground,
        Box::new(|_sink| {
            Err(BackendFailure {
                operation: "dlopen",
                code: 2,
                detail: "libcamera2ndk.so не найдена".to_owned(),
            })
        }),
    )
    .expect("поток запускается");
    let status = wait_state(&handle, "error");
    assert!(matches!(
        status.state,
        CameraState::Error {
            error: CameraError::Fatal {
                cause: FatalCause::BackendUnavailable { .. }
            }
        }
    ));
    let id = handle.open(wide()).expect("принята");
    handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled == 1)
        .expect("ответ получен");
    assert_eq!(
        outcome_of(&handle.drain_events(100), id),
        Some(CommandOutcome::InvalidState(InvalidStateReason::EngineFailed))
    );
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn panic_in_the_backend_factory_is_reported_as_backend_unavailable() {
    let (runtime, handle) = CameraRuntime::spawn(
        CameraEngineConfig::default(),
        AppPresence::Foreground,
        Box::new(|_sink| std::panic::resume_unwind(Box::new("фабрика упала"))),
    )
    .expect("поток запускается");
    let status = wait_state(&handle, "error");
    let CameraState::Error {
        error: CameraError::Fatal {
            cause: FatalCause::BackendUnavailable { failure },
        },
    } = status.state
    else {
        panic!("ожидался Fatal::BackendUnavailable");
    };
    assert!(failure.detail.contains("фабрика упала"));
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn shutdown_closes_the_device_and_later_commands_are_refused_as_engine_stopped() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    start_stream(&handle);
    assert!(fake.is_device_open());
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
    assert!(!fake.is_device_open(), "устройство закрыто при остановке");
    assert_eq!(handle.open(wide()), Err(CommandRejected::EngineStopped));
    assert_eq!(handle.status().state.name(), "closed", "итог остановки виден в снимке");
}

#[test]
fn shutdown_of_a_stuck_thread_times_out_instead_of_hanging() {
    let fake = FakeBackendControl::new();
    let (factory, release, entered) = gated_factory(&fake);
    let (runtime, _handle) =
        CameraRuntime::spawn(CameraEngineConfig::default(), AppPresence::Foreground, factory).expect("запуск");
    entered.recv_timeout(WAIT).expect("поток дошёл до ворот");
    let start = std::time::Instant::now();
    assert_eq!(runtime.shutdown(Duration::from_millis(80)), ShutdownReport::TimedOut);
    assert!(start.elapsed() < Duration::from_secs(5));
    // Отпускаем ворота: отпущенный поток сам доработает и выйдет.
    drop(release);
}

#[test]
fn dropping_the_runtime_without_shutdown_still_stops_the_thread_and_closes_the_device() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    start_stream(&handle);
    drop(runtime);
    assert!(!fake.is_device_open());
    assert_eq!(handle.close(), Err(CommandRejected::EngineStopped));
}

#[test]
fn invalid_config_is_rejected_before_any_thread_is_started() {
    let fake = FakeBackendControl::new();
    let mut config = CameraEngineConfig::default();
    config.queues.command_capacity = 0;
    let result = CameraRuntime::spawn(config, AppPresence::Foreground, fake.factory());
    assert!(matches!(result, Err(RuntimeSpawnError::InvalidConfig(_))));
    assert!(fake.calls().is_empty(), "бэкенд не создавался");
}

#[test]
fn outward_event_overflow_drops_oldest_and_counts_them_in_the_status() {
    let fake = FakeBackendControl::new();
    let mut config = CameraEngineConfig::default();
    config.queues.outward_event_capacity = 4;
    let (runtime, handle) = spawn_with(config, &fake);
    for _ in 0..10 {
        handle.close().expect("принята");
    }
    let status = handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled == 10)
        .expect("все команды исполнены");
    assert_eq!(status.counters.dropped_outward_events, 6);
    let events = handle.drain_events(100);
    assert_eq!(events.len(), 4, "остались последние четыре");
    assert!(
        events
            .iter()
            .all(|event| matches!(event, CameraEvent::CommandHandled { .. }))
    );
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn handle_works_from_other_threads() {
    let fake = FakeBackendControl::new();
    let (runtime, handle) = spawn_default(&fake);
    let from_thread = handle.clone();
    let sender = std::thread::spawn(move || from_thread.open(wide()));
    let id = sender.join().expect("поток").expect("принята");
    handle
        .wait_until_status(WAIT, |status| status.state.name() == "opened")
        .expect("камера открыта по команде из другого потока");
    assert_eq!(
        outcome_of(&handle.drain_events(100), id),
        Some(CommandOutcome::Accepted)
    );
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

// ---- Намерения объектива через настоящий поток (сессия 13) ----

#[test]
fn published_lens_intent_wakes_the_camera_thread_and_switches_the_lens() {
    use camera_contract::ids::LensFacing;
    use camera_contract::zoom::{LensIntent, LensIntentChannel, ZoomRatio};

    let fake = FakeBackendControl::new();
    let intents = LensIntentChannel::new();
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a);
    let (_runtime, handle) = CameraRuntime::spawn_with_lens_wiring(
        config,
        AppPresence::Foreground,
        fake.factory(),
        wiring,
        crate::lens_wiring::LensWiring::new(intents.clone()),
    )
    .expect("поток камеры запускается");
    start_stream(&handle);
    assert_eq!(
        handle.status().lens.active.map(|active| active.role),
        Some(LensRole::Wide)
    );

    let (_, delivery) = intents.publish(LensIntent {
        facing: LensFacing::Back,
        ratio: ZoomRatio::new(0.6).expect("коэффициент"),
        gesture: camera_contract::zoom::ZoomGesture::Continuous,
        standby: camera_contract::zoom::StandbyDemand::NotNeeded,
    });
    assert_eq!(delivery, camera_contract::auto_exposure::SignalDelivery::Delivered);

    let status = handle
        .wait_until_status(WAIT, |status| {
            status.lens.active.as_ref().map(|active| active.role) == Some(LensRole::UltraWide)
        })
        .expect("поток камеры проснулся по толчку и сменил объектив");
    assert_eq!(status.state.name(), "streaming");
    assert_eq!(fake.open_camera(), Some(camera_id("3")));
}
