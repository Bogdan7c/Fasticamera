//! Тесты исполнителя эффектов экрана: каждый вид эффекта доходит до своего сервиса; отказы (очередь полна,
//! движка нет, оболочка не подключена, неверный прямоугольник, снимок отвергнут) — счётчики и обратная связь
//! экрану, а не паника и не тишина.

use std::sync::Mutex;

use camera_contract::control::ExposureBias;
use camera_contract::three_a::SensorPoint;
use camera_screen::settings::{AspectRatio, FlashMode, RawCapture};
use camera_screen::shutter::ShotRequestId;
use camera_screen::{LayoutConfig, ScreenLayout};
use render_loop::ui_overlay::UiOverlayCommand;
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{FrameRate, FrameRateOutcome, ScreenWake};
use shell_contract::haptics::HapticRealizationKind;
use shell_contract::surface::SurfaceSize;

use super::*;
use crate::gpu_test_support::RecordingRenderControl;

/// Порт камеры: запоминает намерения и отвечает заданным итогом.
struct FakeCameraPort {
    outcome: Arc<Mutex<CameraIntentOutcome>>,
    received: Arc<Mutex<Vec<ScreenCameraIntent>>>,
}

impl CameraIntentPort for FakeCameraPort {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        self.received.lock().expect("замок").push(intent);
        *self.outcome.lock().expect("замок")
    }
}

struct FakeHaptics {
    outcome: HapticOutcome,
    played: Mutex<Vec<HapticPattern>>,
}

impl HapticsService for FakeHaptics {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        self.played.lock().expect("замок").push(pattern);
        self.outcome
    }
}

struct FakeCommands {
    gallery: CommandOutcome,
    gallery_opened: Mutex<u32>,
}

impl ShellCommands for FakeCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        *self.gallery_opened.lock().expect("замок") += 1;
        self.gallery
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

/// Сервис снимка с заданным ответом и журналом.
struct StubCapture {
    outcome: SubmitOutcome,
    requests: Mutex<Vec<ShotRequest>>,
}

impl CaptureService for StubCapture {
    fn submit(&self, request: ShotRequest, _now: MonotonicNanos) -> SubmitOutcome {
        self.requests.lock().expect("замок").push(request);
        self.outcome
    }
}

/// Управление рендером с заданным итогом доставки.
struct FixedDeliveryControl(fn() -> ControlDelivery);

impl RenderControl for FixedDeliveryControl {
    fn adjust(&self, _adjustment: RenderAdjustment) -> ControlDelivery {
        (self.0)()
    }

    fn ui_overlay(&self, _command: UiOverlayCommand) -> ControlDelivery {
        (self.0)()
    }
}

/// Всё, что тест наблюдает снаружи исполнителя.
struct Rig {
    intents: Arc<Mutex<Vec<ScreenCameraIntent>>>,
    /// Итог, которым порт отвечает СЕЙЧАС (тест меняет его по ходу: очередь освободилась).
    camera_outcome: Arc<Mutex<CameraIntentOutcome>>,
    haptics: Arc<FakeHaptics>,
    commands: Arc<FakeCommands>,
    capture: Arc<StubCapture>,
    render: Arc<RecordingRenderControl>,
}

struct Behavior {
    camera: CameraIntentOutcome,
    haptics: HapticOutcome,
    gallery: CommandOutcome,
    capture: SubmitOutcome,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            camera: CameraIntentOutcome::Sent(CommandId(1)),
            haptics: HapticOutcome::Queued(HapticRealizationKind::Primitive),
            gallery: CommandOutcome::Queued,
            capture: SubmitOutcome::Accepted,
        }
    }
}

fn build_executor(behavior: Behavior) -> (EffectExecutor, Rig) {
    let intents = Arc::new(Mutex::new(Vec::new()));
    let haptics = Arc::new(FakeHaptics {
        outcome: behavior.haptics,
        played: Mutex::default(),
    });
    let commands = Arc::new(FakeCommands {
        gallery: behavior.gallery,
        gallery_opened: Mutex::default(),
    });
    let capture = Arc::new(StubCapture {
        outcome: behavior.capture,
        requests: Mutex::default(),
    });
    let render = RecordingRenderControl::new();
    let camera_outcome = Arc::new(Mutex::new(behavior.camera));
    let executor = EffectExecutor::new(EffectTargets {
        camera: Box::new(FakeCameraPort {
            outcome: Arc::clone(&camera_outcome),
            received: Arc::clone(&intents),
        }),
        haptics: haptics.clone(),
        commands: commands.clone(),
        render: render.clone(),
        capture: capture.clone(),
    });
    (
        executor,
        Rig {
            intents,
            camera_outcome,
            haptics,
            commands,
            capture,
            render,
        },
    )
}

fn shot(id: u64) -> ShotRequest {
    ShotRequest {
        request: ShotRequestId(id),
        flash: FlashMode::Off,
        raw: RawCapture::Off,
        aspect: AspectRatio::FourByThree,
    }
}

fn layout_framing() -> FramingIntent {
    ScreenLayout::new(
        &LayoutConfig::default(),
        SurfaceSize {
            width_px: 1440,
            height_px: 3216,
        },
    )
    .expect("раскладка")
    .framing_intent(AspectRatio::FourByThree)
}

#[test]
fn every_camera_intent_reaches_the_camera_port_in_order() {
    let (mut executor, rig) = build_executor(Behavior::default());
    let point = SensorPoint::new(0.25, 0.5).expect("точка");
    let bias = ExposureBias::from_ev(0.5).expect("EV");
    let intents = [
        ScreenCameraIntent::FocusAndExposeAt(point),
        ScreenCameraIntent::LockFocusAndExposureAt(point),
        ScreenCameraIntent::ResetPointOfInterest,
        ScreenCameraIntent::SetExposureBias(bias),
    ];

    let feedback = executor.execute(intents.iter().copied().map(Effect::Camera).collect());

    assert!(feedback.is_empty());
    assert_eq!(*rig.intents.lock().expect("замок"), intents);
    assert_eq!(executor.counters().camera.sent, 4);
}

#[test]
fn camera_refusals_are_counted_by_kind() {
    for (outcome, check) in [
        (
            CameraIntentOutcome::QueueFull,
            (|c: CameraIntentCounters| c.queue_full) as fn(CameraIntentCounters) -> u64,
        ),
        (CameraIntentOutcome::EngineStopped, |c| c.engine_stopped),
        (CameraIntentOutcome::NoEngine, |c| c.no_engine),
    ] {
        let (mut executor, _rig) = build_executor(Behavior {
            camera: outcome,
            ..Behavior::default()
        });
        executor.execute(vec![Effect::Camera(ScreenCameraIntent::ResetPointOfInterest)]);
        let counters = executor.counters().camera;
        assert_eq!(check(counters), 1, "{outcome:?}: {counters:?}");
        assert_eq!(counters.sent, 0, "{outcome:?}");
    }
}

#[test]
fn haptic_effect_plays_the_pattern_and_counts_each_outcome() {
    let (mut executor, rig) = build_executor(Behavior::default());
    executor.execute(vec![Effect::Haptic(HapticPattern::MediumImpact)]);
    assert_eq!(
        *rig.haptics.played.lock().expect("замок"),
        [HapticPattern::MediumImpact]
    );
    assert_eq!(executor.counters().haptics.queued, 1);

    for (outcome, check) in [
        (
            HapticOutcome::Unsupported,
            (|c: HapticCounters| c.unsupported) as fn(HapticCounters) -> u64,
        ),
        (HapticOutcome::NotAttached, |c| c.not_attached),
        (HapticOutcome::Backpressure, |c| c.backpressure),
    ] {
        let (mut executor, _rig) = build_executor(Behavior {
            haptics: outcome,
            ..Behavior::default()
        });
        executor.execute(vec![Effect::Haptic(HapticPattern::LightImpact)]);
        assert_eq!(check(executor.counters().haptics), 1, "{outcome:?}");
    }
}

#[test]
fn gallery_effect_opens_the_system_gallery_and_counts_refusals() {
    let (mut executor, rig) = build_executor(Behavior::default());
    executor.execute(vec![Effect::OpenSystemGallery]);
    assert_eq!(*rig.commands.gallery_opened.lock().expect("замок"), 1);
    assert_eq!(executor.counters().gallery.queued, 1);

    let (mut detached, _rig) = build_executor(Behavior {
        gallery: CommandOutcome::NotAttached,
        ..Behavior::default()
    });
    detached.execute(vec![Effect::OpenSystemGallery]);
    assert_eq!(detached.counters().gallery.not_attached, 1);
    let (mut busy, _rig) = build_executor(Behavior {
        gallery: CommandOutcome::Backpressure,
        ..Behavior::default()
    });
    busy.execute(vec![Effect::OpenSystemGallery]);
    assert_eq!(busy.counters().gallery.backpressure, 1);
}

#[test]
fn viewfinder_effect_becomes_a_set_framing_command_with_the_screens_rectangle() {
    let (mut executor, rig) = build_executor(Behavior::default());
    let intent = layout_framing();

    executor.execute(vec![Effect::Viewfinder(intent)]);

    let recorded = rig.render.recorded();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    let RenderAdjustment::Viewfinder(ViewfinderCommand::SetFraming(framing)) = recorded[0] else {
        panic!("ожидали SetFraming, пришло {:?}", recorded[0]);
    };
    assert_eq!(framing.crop, intent.crop);
    let FramingDestination::ScreenRect(rect) = framing.destination else {
        panic!("ожидали прямоугольник экрана, пришло {:?}", framing.destination);
    };
    assert_eq!(
        (rect.left(), rect.top(), rect.width(), rect.height()),
        (
            intent.destination.left,
            intent.destination.top,
            intent.destination.width,
            intent.destination.height
        ),
        "числа прямоугольника переходят без искажений"
    );
    assert_eq!(executor.counters().framing.delivered, 1);
}

#[test]
fn an_invalid_rectangle_is_counted_and_never_sent_to_render() {
    let (mut executor, rig) = build_executor(Behavior::default());
    let mut intent = layout_framing();
    intent.destination.width = 1.5;

    executor.execute(vec![Effect::Viewfinder(intent)]);

    assert!(rig.render.recorded().is_empty(), "неверная команда рендеру не уходит");
    let counters = executor.counters().framing;
    assert_eq!((counters.invalid_rect, counters.delivered), (1, 0));
}

#[test]
fn framing_delivery_failures_are_counted_by_kind() {
    for (make, check) in [
        (
            (|| ControlDelivery::RenderGone) as fn() -> ControlDelivery,
            (|c: FramingCounters| c.render_gone) as fn(FramingCounters) -> u64,
        ),
        (
            || ControlDelivery::RenderUnavailable {
                reason: "нет GPU".to_owned(),
            },
            |c| c.render_unavailable,
        ),
    ] {
        let (mut executor, rig) = build_executor(Behavior::default());
        executor.targets.render = Arc::new(FixedDeliveryControl(make));
        executor.execute(vec![Effect::Viewfinder(layout_framing())]);
        assert_eq!(check(executor.counters().framing), 1);
        assert!(rig.render.recorded().is_empty());
    }
}

#[test]
fn accepted_capture_reaches_the_service_without_feedback() {
    let (mut executor, rig) = build_executor(Behavior::default());

    let feedback = executor.execute(vec![Effect::Capture(shot(9))]);

    assert!(feedback.is_empty(), "ответ придёт позже, от сервиса");
    assert_eq!(*rig.capture.requests.lock().expect("замок"), [shot(9)]);
    assert_eq!(executor.counters().capture.accepted, 1);
}

#[test]
fn rejected_capture_gives_the_screen_a_failure_instead_of_leaving_the_shutter_busy() {
    for (rejection, expected_reason) in [
        (SubmitRejection::Busy, CaptureFailureReason::Processing),
        (SubmitRejection::ServiceUnavailable, CaptureFailureReason::Camera),
    ] {
        let (mut executor, _rig) = build_executor(Behavior {
            capture: SubmitOutcome::Rejected(rejection),
            ..Behavior::default()
        });

        let feedback = executor.execute(vec![Effect::Capture(shot(4))]);

        assert_eq!(
            feedback,
            vec![ScreenAction::CaptureFinished {
                request: ShotRequestId(4),
                outcome: CaptureOutcome::Failed(expected_reason),
            }],
            "{rejection:?}"
        );
        let counters = executor.counters().capture;
        assert_eq!(counters.accepted, 0);
        assert_eq!(counters.rejected_busy + counters.rejected_unavailable, 1);
    }
}

#[test]
fn a_mixed_batch_is_executed_in_order_and_touches_only_its_own_services() {
    let (mut executor, rig) = build_executor(Behavior::default());

    executor.execute(vec![
        Effect::Haptic(HapticPattern::SelectionTick),
        Effect::Camera(ScreenCameraIntent::ResetPointOfInterest),
        Effect::Haptic(HapticPattern::LightImpact),
    ]);

    assert_eq!(
        *rig.haptics.played.lock().expect("замок"),
        [HapticPattern::SelectionTick, HapticPattern::LightImpact]
    );
    assert_eq!(rig.intents.lock().expect("замок").len(), 1);
    assert!(rig.capture.requests.lock().expect("замок").is_empty());
    assert_eq!(*rig.commands.gallery_opened.lock().expect("замок"), 0);
    assert!(rig.render.recorded().is_empty());
}

fn set_bias(ev: f32) -> ScreenCameraIntent {
    ScreenCameraIntent::SetExposureBias(ExposureBias::from_ev(ev).expect("EV"))
}

fn bias_effect(ev: f32) -> Effect {
    Effect::Camera(set_bias(ev))
}

#[test]
fn an_exposure_bias_refused_by_a_full_queue_is_resent_until_accepted_and_the_last_value_wins() {
    let (mut executor, rig) = build_executor(Behavior {
        camera: CameraIntentOutcome::QueueFull,
        ..Behavior::default()
    });
    executor.execute(vec![bias_effect(0.5)]);
    executor.execute(vec![bias_effect(1.0)]);
    assert_eq!(
        executor.pending_exposure_bias(),
        ExposureBias::from_ev(1.0).ok(),
        "последнее желаемое значение ждёт"
    );

    // Очередь всё ещё полна: досылка не проходит, значение остаётся.
    executor.resend_pending_exposure_bias();
    assert_eq!(executor.counters().camera.exposure_resends_queue_full, 1);
    assert!(executor.pending_exposure_bias().is_some());

    // Очередь освободилась: ушло именно ПОСЛЕДНЕЕ значение, ожидание снято.
    *rig.camera_outcome.lock().expect("замок") = CameraIntentOutcome::Sent(CommandId(7));
    executor.resend_pending_exposure_bias();
    assert_eq!(rig.intents.lock().expect("замок").last().copied(), Some(set_bias(1.0)));
    assert_eq!(executor.counters().camera.exposure_resends_accepted, 1);
    assert_eq!(executor.pending_exposure_bias(), None);
    // Принятая досылка — не новое намерение экрана.
    assert_eq!(executor.counters().camera.sent, 0);

    // Больше досылать нечего: порт больше не вызывается.
    let calls = rig.intents.lock().expect("замок").len();
    executor.resend_pending_exposure_bias();
    assert_eq!(rig.intents.lock().expect("замок").len(), calls);
}

#[test]
fn a_fresh_accepted_exposure_bias_cancels_the_pending_resend() {
    let (mut executor, rig) = build_executor(Behavior {
        camera: CameraIntentOutcome::QueueFull,
        ..Behavior::default()
    });
    executor.execute(vec![bias_effect(0.5)]);
    *rig.camera_outcome.lock().expect("замок") = CameraIntentOutcome::Sent(CommandId(2));
    executor.execute(vec![bias_effect(-1.0)]);
    assert_eq!(executor.pending_exposure_bias(), None, "старое значение устарело");
    let calls = rig.intents.lock().expect("замок").len();
    executor.resend_pending_exposure_bias();
    assert_eq!(rig.intents.lock().expect("замок").len(), calls, "досылать нечего");
}

#[test]
fn focus_and_lock_intents_refused_by_a_full_queue_are_never_resent() {
    let (mut executor, rig) = build_executor(Behavior {
        camera: CameraIntentOutcome::QueueFull,
        ..Behavior::default()
    });
    let point = SensorPoint::new(0.5, 0.5).expect("точка");
    executor.execute(vec![
        Effect::Camera(ScreenCameraIntent::FocusAndExposeAt(point)),
        Effect::Camera(ScreenCameraIntent::LockFocusAndExposureAt(point)),
        Effect::Camera(ScreenCameraIntent::ResetPointOfInterest),
    ]);
    assert_eq!(executor.pending_exposure_bias(), None);
    *rig.camera_outcome.lock().expect("замок") = CameraIntentOutcome::Sent(CommandId(3));
    let calls = rig.intents.lock().expect("замок").len();
    executor.resend_pending_exposure_bias();
    assert_eq!(
        rig.intents.lock().expect("замок").len(),
        calls,
        "устаревшее не досылается"
    );
    assert_eq!(executor.counters().camera.queue_full, 3);
}

#[test]
fn a_resend_to_a_stopped_or_absent_engine_is_abandoned_with_a_counter() {
    for outcome in [CameraIntentOutcome::EngineStopped, CameraIntentOutcome::NoEngine] {
        let (mut executor, rig) = build_executor(Behavior {
            camera: CameraIntentOutcome::QueueFull,
            ..Behavior::default()
        });
        executor.execute(vec![bias_effect(0.5)]);
        *rig.camera_outcome.lock().expect("замок") = outcome;
        executor.resend_pending_exposure_bias();
        assert_eq!(executor.pending_exposure_bias(), None, "{outcome:?}");
        assert_eq!(executor.counters().camera.exposure_resends_abandoned, 1, "{outcome:?}");
    }
}
