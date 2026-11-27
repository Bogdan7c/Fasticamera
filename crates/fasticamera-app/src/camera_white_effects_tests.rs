//! Тесты исполнителя белых намерений: досылка при полной очереди (последнее побеждает, по осям, порядок), сброс
//! при остановленном движке, «взять» не досылается и трясёт экран, а не молчит.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::white::{WhiteAdjustment, WhiteSourceId};
use camera_core::CommandId;
use camera_screen::viewfinder_map::{MirrorMode, StreamGeometry};
use camera_screen::{CameraFacts, CameraPresence, CameraScreen, ScreenAction, ScreenCameraIntent, ScreenConfig};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;

use super::*;
use crate::camera_screen_capture::{CaptureService, SubmitOutcome};
use crate::camera_screen_effects::{EffectExecutor, EffectTargets};
use crate::camera_screen_facts::{CameraFactsSource, FactsDiagnostics};
use crate::camera_screen_input::screen_input_channel;
use crate::camera_screen_source::{CameraScreenSource, GravityLink, ScreenBoard, ScreenSourceParts};
use crate::gpu_test_support::RecordingRenderControl;

const FULL: CameraIntentOutcome = CameraIntentOutcome::QueueFull;
const ACCEPTED: CameraIntentOutcome = CameraIntentOutcome::Sent(CommandId(1));

/// Порт камеры: отвечает по сценарию (по одному ответу на вызов), потом — постоянным ответом; помнит всё принятое.
struct ScriptedPort {
    script: Mutex<VecDeque<CameraIntentOutcome>>,
    fallback: Mutex<CameraIntentOutcome>,
    received: Mutex<Vec<ScreenCameraIntent>>,
}

impl ScriptedPort {
    fn answering(outcome: CameraIntentOutcome) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::default(),
            fallback: Mutex::new(outcome),
            received: Mutex::default(),
        })
    }

    fn scripted(script: &[CameraIntentOutcome], then: CameraIntentOutcome) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.iter().copied().collect()),
            fallback: Mutex::new(then),
            received: Mutex::default(),
        })
    }

    fn answer_from_now_on(&self, outcome: CameraIntentOutcome) {
        *self.fallback.lock().expect("замок") = outcome;
    }

    fn received(&self) -> Vec<ScreenCameraIntent> {
        self.received.lock().expect("замок").clone()
    }
}

impl CameraIntentPort for Arc<ScriptedPort> {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        // Пауза слежения — отдельный уровень (`camera_white_hold`): ни записи, ни траты сценария ответов.
        if matches!(intent, ScreenCameraIntent::HoldWhiteTracking(_)) {
            return ACCEPTED;
        }
        self.received.lock().expect("замок").push(intent);
        let scripted = self.script.lock().expect("замок").pop_front();
        scripted.unwrap_or_else(|| *self.fallback.lock().expect("замок"))
    }
}

fn kelvin(value: f64) -> ColorTemperature {
    ColorTemperature::from_kelvin(value).expect("температура")
}

fn duv(value: f64) -> Tint {
    Tint::from_duv(value).expect("оттенок")
}

fn setting(kelvin_value: f64, duv_value: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: kelvin(kelvin_value),
        tint: duv(duv_value),
    }
}

fn set(kelvin_value: f64, duv_value: f64) -> ScreenCameraIntent {
    ScreenCameraIntent::SetWhite(setting(kelvin_value, duv_value))
}

fn temperature_edit(kelvin_value: f64) -> ScreenCameraIntent {
    ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Temperature(kelvin(kelvin_value)))
}

fn tint_edit(duv_value: f64) -> ScreenCameraIntent {
    ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(duv(duv_value)))
}

fn take() -> ScreenCameraIntent {
    ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)
}

fn follow() -> ScreenCameraIntent {
    ScreenCameraIntent::FollowWhite(WhiteSourceId::PHONE)
}

/// Исполнитель с порталом на `ScriptedPort`.
fn executor_with(port: &Arc<ScriptedPort>) -> EffectExecutor {
    EffectExecutor::new(EffectTargets {
        camera: Box::new(Arc::clone(port)),
        haptics: Arc::new(RecordingHaptics::default()),
        commands: Arc::new(QuietCommands),
        render: RecordingRenderControl::new(),
        capture: Arc::new(NoCapture),
        shutter_sound: Arc::new(crate::camera_screen_service::SilentShutterSound::default()),
        lens: camera_contract::zoom::LensIntentChannel::new(),
        look: None,
    })
}

fn run(executor: &mut EffectExecutor, intent: ScreenCameraIntent) -> Vec<ScreenAction> {
    executor.execute(vec![camera_screen::Effect::Camera(intent)])
}

// ---- досылка ----

#[test]
fn a_manual_white_refused_by_a_full_queue_is_resent_until_accepted_and_the_last_one_wins() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    run(&mut executor, set(5_600.0, 0.01));
    assert_eq!(executor.pending_white().whole(), Some(setting(5_600.0, 0.01)));

    // Очередь всё ещё полна: остаётся ждать.
    executor.resend_pending_white();
    assert_eq!(executor.pending_white().whole(), Some(setting(5_600.0, 0.01)));
    assert_eq!(executor.counters().white.resends_accepted, 0);

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(
        port.received()[before..],
        [set(5_600.0, 0.01)],
        "уходит только последнее значение"
    );
    assert!(executor.pending_white().is_empty());
    assert_eq!(executor.counters().white.resends_accepted, 1);

    executor.resend_pending_white();
    assert_eq!(port.received().len(), before + 1, "принятое больше не досылается");
}

#[test]
fn pending_temperature_and_tint_live_independently_and_each_keeps_its_last_value() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, temperature_edit(4_000.0));
    run(&mut executor, tint_edit(0.02));
    run(&mut executor, temperature_edit(4_500.0));
    let pending = executor.pending_white();
    assert_eq!(pending.temperature(), Some(kelvin(4_500.0)));
    assert_eq!(pending.tint(), Some(duv(0.02)), "правка температуры не затёрла оттенок");
    assert_eq!(pending.whole(), None);

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(port.received()[before..], [temperature_edit(4_500.0), tint_edit(0.02)]);
    assert!(executor.pending_white().is_empty());
}

#[test]
fn an_accepted_axis_leaves_nothing_pending_while_the_other_axis_waits_alone() {
    // Температуру принимает движок, оттенок упирается в очередь: ждёт ровно оттенок.
    let port = ScriptedPort::scripted(&[ACCEPTED], FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, temperature_edit(4_000.0));
    assert!(executor.pending_white().is_empty());
    run(&mut executor, tint_edit(0.02));
    let pending = executor.pending_white();
    assert_eq!((pending.temperature(), pending.tint()), (None, Some(duv(0.02))));
}

#[test]
fn a_set_white_replaces_both_pending_axes() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, temperature_edit(4_000.0));
    run(&mut executor, tint_edit(0.02));
    run(&mut executor, set(6_500.0, -0.01));
    let pending = executor.pending_white();
    assert_eq!(pending.whole(), Some(setting(6_500.0, -0.01)));
    assert_eq!((pending.temperature(), pending.tint()), (None, None));

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(
        port.received()[before..],
        [set(6_500.0, -0.01)],
        "старые оси не воскресли поверх кнопки света"
    );
}

#[test]
fn an_axis_edit_after_a_pending_set_white_lands_on_top_of_it_and_keeps_the_order() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(5_600.0, 0.0));
    run(&mut executor, temperature_edit(3_900.0));
    run(&mut executor, tint_edit(0.03));
    assert_eq!(executor.pending_white().whole(), Some(setting(3_900.0, 0.03)));

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(
        port.received()[before..],
        [set(3_900.0, 0.03)],
        "одна команда: кнопка света с правками поверх"
    );
}

#[test]
fn a_new_edit_never_overtakes_an_older_pending_one() {
    // Старый SetWhite ждёт; очередь освободилась как раз к новой правке: до движка обе дойдут по порядку.
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, temperature_edit(3_000.0));
    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    run(&mut executor, tint_edit(-0.02));
    assert_eq!(port.received()[before..], [temperature_edit(3_000.0), tint_edit(-0.02)]);
    assert!(executor.pending_white().is_empty());
}

#[test]
fn a_stopped_or_missing_engine_drops_everything_pending_and_counts_it() {
    for (outcome, expect_stopped, expect_missing) in [
        (CameraIntentOutcome::EngineStopped, 1, 0),
        (CameraIntentOutcome::NoEngine, 0, 1),
    ] {
        let port = ScriptedPort::answering(FULL);
        let mut executor = executor_with(&port);
        run(&mut executor, temperature_edit(4_000.0));
        run(&mut executor, tint_edit(0.02));
        assert!(!executor.pending_white().is_empty());

        port.answer_from_now_on(outcome);
        executor.resend_pending_white();
        assert!(executor.pending_white().is_empty(), "{outcome:?}");
        let counters = executor.counters().white;
        assert_eq!(
            (counters.engine_stopped, counters.no_engine),
            (expect_stopped, expect_missing)
        );

        // Ничего больше не уходит, даже когда движок вернулся.
        port.answer_from_now_on(ACCEPTED);
        let before = port.received().len();
        executor.resend_pending_white();
        assert_eq!(port.received().len(), before, "{outcome:?}: нечего досылать");
    }
}

// ---- «взять» ----

#[test]
fn a_take_that_did_not_reach_the_engine_goes_back_to_the_screen_as_a_failure_and_is_never_resent() {
    for outcome in [
        CameraIntentOutcome::QueueFull,
        CameraIntentOutcome::EngineStopped,
        CameraIntentOutcome::NoEngine,
    ] {
        let port = ScriptedPort::answering(outcome);
        let mut executor = executor_with(&port);
        let feedback = run(&mut executor, take());
        assert_eq!(feedback, vec![ScreenAction::WhiteTakeNotDelivered], "{outcome:?}");
        assert_eq!(executor.counters().white.takes_not_delivered, 1, "{outcome:?}");

        port.answer_from_now_on(ACCEPTED);
        executor.resend_pending_white();
        assert_eq!(port.received(), vec![take()], "{outcome:?}: «взять» не повторяется");
    }
}

#[test]
fn an_accepted_take_returns_no_feedback_and_counts_as_sent() {
    let port = ScriptedPort::answering(ACCEPTED);
    let mut executor = executor_with(&port);
    assert_eq!(run(&mut executor, take()), Vec::<ScreenAction>::new());
    assert_eq!(executor.counters().white.sent, 1);
    assert_eq!(executor.counters().white.takes_not_delivered, 0);
}

#[test]
fn a_pending_manual_edit_reaches_the_engine_before_a_take_pressed_after_it() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    // Очередь освободилась: в кадре сперва досылка отложенного, потом нажатие «взять».
    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    let feedback = run(&mut executor, take());
    assert_eq!(feedback, Vec::<ScreenAction>::new());
    assert_eq!(port.received()[before..], [set(3_200.0, 0.0), take()]);
}

#[test]
fn an_accepted_take_drops_the_older_pending_edit_so_the_hand_does_not_land_on_top_of_it() {
    // Правка отказана, повтор отказан, попытка досылки перед «взять» отказана, а сама «взять» принята.
    let port = ScriptedPort::scripted(&[FULL, FULL, FULL, ACCEPTED], ACCEPTED);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    executor.resend_pending_white();
    assert!(!executor.pending_white().is_empty());
    assert_eq!(run(&mut executor, take()), Vec::<ScreenAction>::new());
    assert!(executor.pending_white().is_empty());

    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(port.received().len(), before, "старая рука не воскресла");
}

#[test]
fn a_refused_take_leaves_the_pending_manual_edit_alone() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    let feedback = run(&mut executor, take());
    assert_eq!(feedback, vec![ScreenAction::WhiteTakeNotDelivered]);
    assert_eq!(executor.pending_white().whole(), Some(setting(3_200.0, 0.0)));

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(port.received()[before..], [set(3_200.0, 0.0)]);
}

// ---- «следить» («Auto», WB44) ----

#[test]
fn a_follow_that_did_not_reach_the_engine_goes_back_as_its_own_failure_is_counted_and_never_resent() {
    for (outcome, reason_counter) in [
        (CameraIntentOutcome::QueueFull, "queue_full"),
        (CameraIntentOutcome::EngineStopped, "engine_stopped"),
        (CameraIntentOutcome::NoEngine, "no_engine"),
    ] {
        let port = ScriptedPort::answering(outcome);
        let mut executor = executor_with(&port);
        let feedback = run(&mut executor, follow());
        assert_eq!(
            feedback,
            vec![ScreenAction::WhiteFollowNotDelivered],
            "{outcome:?}: не «отказ взять»: «Phone» трястись не должна"
        );
        let counters = executor.counters().white;
        assert_eq!(counters.follows_not_delivered, 1, "{outcome:?}");
        assert_eq!(counters.takes_not_delivered, 0, "{outcome:?}: чужой счётчик не тронут");
        assert_eq!(counters.sent, 0, "{outcome:?}");
        let reason = match reason_counter {
            "queue_full" => counters.queue_full,
            "engine_stopped" => counters.engine_stopped,
            _ => counters.no_engine,
        };
        assert_eq!(reason, 1, "{outcome:?}: причина посчитана в «{reason_counter}»");

        port.answer_from_now_on(ACCEPTED);
        executor.resend_pending_white();
        assert_eq!(port.received(), vec![follow()], "{outcome:?}: «следить» не повторяется");
    }
}

#[test]
fn an_accepted_follow_returns_no_feedback_and_counts_as_sent() {
    let port = ScriptedPort::answering(ACCEPTED);
    let mut executor = executor_with(&port);
    assert_eq!(run(&mut executor, follow()), Vec::<ScreenAction>::new());
    assert_eq!(port.received(), vec![follow()]);
    let counters = executor.counters().white;
    assert_eq!((counters.sent, counters.follows_not_delivered), (1, 0));
}

#[test]
fn a_pending_manual_edit_reaches_the_engine_before_a_follow_pressed_after_it() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(run(&mut executor, follow()), Vec::<ScreenAction>::new());
    assert_eq!(port.received()[before..], [set(3_200.0, 0.0), follow()]);
}

#[test]
fn an_accepted_follow_drops_the_older_pending_edit_so_a_stale_hand_does_not_switch_follow_off_again() {
    // Правка отказана, повтор отказан, досылка перед «следить» отказана, сама «следить» принята.
    let port = ScriptedPort::scripted(&[FULL, FULL, FULL, ACCEPTED], ACCEPTED);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    executor.resend_pending_white();
    assert!(!executor.pending_white().is_empty());
    assert_eq!(run(&mut executor, follow()), Vec::<ScreenAction>::new());
    assert!(executor.pending_white().is_empty(), "отложенная рука сброшена");

    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(
        port.received().len(),
        before,
        "старая рука не воскресла и слежение не выключила"
    );
}

#[test]
fn a_follow_refused_by_a_full_queue_leaves_the_pending_manual_edit_alone_and_does_not_resend_itself() {
    let port = ScriptedPort::answering(FULL);
    let mut executor = executor_with(&port);
    run(&mut executor, set(3_200.0, 0.0));
    let feedback = run(&mut executor, follow());
    assert_eq!(feedback, vec![ScreenAction::WhiteFollowNotDelivered]);
    assert_eq!(
        executor.pending_white().whole(),
        Some(setting(3_200.0, 0.0)),
        "рука ждёт своего"
    );

    port.answer_from_now_on(ACCEPTED);
    let before = port.received().len();
    executor.resend_pending_white();
    assert_eq!(port.received()[before..], [set(3_200.0, 0.0)], "досылается только рука");
}

#[test]
fn a_stopped_or_missing_engine_drops_the_pending_edit_when_a_follow_is_pressed() {
    for outcome in [CameraIntentOutcome::EngineStopped, CameraIntentOutcome::NoEngine] {
        let port = ScriptedPort::answering(FULL);
        let mut executor = executor_with(&port);
        run(&mut executor, set(3_200.0, 0.0));
        port.answer_from_now_on(outcome);
        assert_eq!(
            run(&mut executor, follow()),
            vec![ScreenAction::WhiteFollowNotDelivered],
            "{outcome:?}"
        );
        assert!(
            executor.pending_white().is_empty(),
            "{outcome:?}: движка нет — рука сброшена"
        );
    }
}

// ---- сквозной путь через источник кадров: экран трясёт ----

#[derive(Default)]
struct RecordingHaptics {
    played: Mutex<Vec<HapticPattern>>,
}

impl HapticsService for RecordingHaptics {
    fn play(&self, pattern: HapticPattern) -> HapticOutcome {
        self.played.lock().expect("замок").push(pattern);
        HapticOutcome::Unsupported
    }
}

struct QuietCommands;

impl ShellCommands for QuietCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

struct NoCapture;

impl CaptureService for NoCapture {
    fn submit(
        &self,
        _request: camera_screen::shutter::ShotRequest,
        _look: look_contract::LookRecipe,
        _now: MonotonicNanos,
    ) -> SubmitOutcome {
        SubmitOutcome::Accepted
    }
}

struct LiveFacts;

impl CameraFactsSource for LiveFacts {
    fn current_facts(&mut self) -> CameraFacts {
        CameraFacts {
            presence: CameraPresence::Live,
            stream: Some(StreamGeometry {
                valid_area: PixelRect {
                    left: 0,
                    top: 0,
                    size: PixelSize::new(4096, 3072),
                },
                active_array_size: PixelSize::new(4096, 3072),
                sensor_orientation: SensorOrientation::Degrees90,
                mirror: MirrorMode::None,
            }),
            ..CameraFacts::STARTING
        }
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        FactsDiagnostics::default()
    }
}

fn frame_request(now_ms: i64) -> UiFrameRequest {
    UiFrameRequest {
        present_time: MonotonicNanos::from_nanos(now_ms * 1_000_000),
        logical_size: PxSize::new(1080.0, 2412.0),
        px_per_pt: 1080.0 / 440.0,
        backdrop: BackdropContinuity::Live,
    }
}

/// Источник кадров с настоящим экраном и исполнителем, у которого камерный порт — `ScriptedPort`.
struct Stand {
    source: CameraScreenSource,
    sender: crate::camera_screen_input::ScreenInputSender,
    haptics: Arc<RecordingHaptics>,
    frame_ms: i64,
}

impl Stand {
    fn new(port: &Arc<ScriptedPort>) -> Self {
        let haptics = Arc::new(RecordingHaptics::default());
        let (sender, receiver) = screen_input_channel(16);
        let mut source = CameraScreenSource::new(ScreenSourceParts {
            look_white_stops: crate::look_white_stops::LookWhiteStopsWatcher::silent(),
            scope_labels: crate::scope_labels::ScopeLabelsWatcher::silent(),
            screen: CameraScreen::new(
                ScreenConfig::default(),
                ui_kit::tokens::UiKitTokens::default(),
                ui_core::time::MotionPreference::Full,
            )
            .expect("экран"),
            input: receiver,
            facts: Box::new(LiveFacts),
            look_facts: None,
            gravity: GravityLink::Unavailable {
                reason: "тест".to_owned(),
            },
            executor: EffectExecutor::new(EffectTargets {
                camera: Box::new(Arc::clone(port)),
                haptics: haptics.clone(),
                commands: Arc::new(QuietCommands),
                render: RecordingRenderControl::new(),
                capture: Arc::new(NoCapture),
                shutter_sound: Arc::new(crate::camera_screen_service::SilentShutterSound::default()),
                lens: camera_contract::zoom::LensIntentChannel::new(),
                look: None,
            }),
            board: Arc::new(ScreenBoard::default()),
            report_min_interval: Duration::ZERO,
        });
        source.produce_frame(&frame_request(16)).expect("кадр");
        Self {
            source,
            sender,
            haptics,
            frame_ms: 16,
        }
    }

    fn frame_with(&mut self, action: ScreenAction) {
        self.sender.send_action(action);
        self.frame_ms += 16;
        self.source.produce_frame(&frame_request(self.frame_ms)).expect("кадр");
    }

    fn error_vibrations(&self) -> usize {
        self.haptics
            .played
            .lock()
            .expect("замок")
            .iter()
            .filter(|pattern| **pattern == HapticPattern::Error)
            .count()
    }

    fn refusal_serial(&self) -> u64 {
        self.source.screen_state().white().refusal_serial()
    }
}

#[test]
fn a_long_press_the_engine_never_got_shakes_the_screen_and_vibrates_an_error_exactly_once() {
    for outcome in [
        CameraIntentOutcome::QueueFull,
        CameraIntentOutcome::EngineStopped,
        CameraIntentOutcome::NoEngine,
    ] {
        let port = ScriptedPort::answering(outcome);
        let mut stand = Stand::new(&port);
        assert_eq!(stand.refusal_serial(), 0);
        stand.frame_with(ScreenAction::WhiteButtonLongPressed);
        assert_eq!(port.received(), vec![take()], "{outcome:?}: намерение дошло до порта");
        assert_eq!(stand.refusal_serial(), 1, "{outcome:?}: экран трясёт");
        // Обратное действие экран разбирает в кадре исполнения, а его эффекты (вибрацию) исполняет следующий кадр.
        stand.frame_with(ScreenAction::ScopeDemandLost);
        assert_eq!(stand.error_vibrations(), 1, "{outcome:?}: вибрация «ошибка» ровно одна");

        // Дальнейшие кадры отказ не повторяют.
        stand.frame_with(ScreenAction::ScopeDemandLost);
        assert_eq!(
            (stand.refusal_serial(), stand.error_vibrations()),
            (1, 1),
            "{outcome:?}"
        );
    }
}

#[test]
fn an_auto_tap_the_engine_never_got_vibrates_an_error_once_and_does_not_shake_phone() {
    for outcome in [
        CameraIntentOutcome::QueueFull,
        CameraIntentOutcome::EngineStopped,
        CameraIntentOutcome::NoEngine,
    ] {
        let port = ScriptedPort::answering(outcome);
        let mut stand = Stand::new(&port);
        stand.frame_with(ScreenAction::WhiteButtonTapped);
        stand.frame_with(ScreenAction::WhiteAutoTapped);
        assert_eq!(port.received(), vec![follow()], "{outcome:?}: намерение дошло до порта");
        assert_eq!(
            stand.refusal_serial(),
            0,
            "{outcome:?}: тряски нет: это не отказ «Phone»"
        );
        stand.frame_with(ScreenAction::ScopeDemandLost);
        assert_eq!(stand.error_vibrations(), 1, "{outcome:?}: вибрация «ошибка» ровно одна");
        stand.frame_with(ScreenAction::ScopeDemandLost);
        assert_eq!(
            (stand.refusal_serial(), stand.error_vibrations()),
            (0, 1),
            "{outcome:?}"
        );
    }
}

#[test]
fn a_delivered_auto_tap_neither_vibrates_nor_shakes() {
    let port = ScriptedPort::answering(ACCEPTED);
    let mut stand = Stand::new(&port);
    stand.frame_with(ScreenAction::WhiteButtonTapped);
    stand.frame_with(ScreenAction::WhiteAutoTapped);
    assert_eq!(port.received(), vec![follow()]);
    stand.frame_with(ScreenAction::ScopeDemandLost);
    assert_eq!((stand.refusal_serial(), stand.error_vibrations()), (0, 0));
}

#[test]
fn a_delivered_long_press_does_not_shake_the_screen() {
    let port = ScriptedPort::answering(ACCEPTED);
    let mut stand = Stand::new(&port);
    stand.frame_with(ScreenAction::WhiteButtonLongPressed);
    assert_eq!(port.received(), vec![take()]);
    // Второй кадр: вибрацию отказа исполнил бы он, но отказа не было.
    stand.frame_with(ScreenAction::ScopeDemandLost);
    assert_eq!((stand.refusal_serial(), stand.error_vibrations()), (0, 0));
}

#[test]
fn a_scale_drag_refused_by_a_full_queue_is_resent_by_the_source_on_later_frames() {
    let port = ScriptedPort::answering(FULL);
    let mut stand = Stand::new(&port);
    stand.frame_with(ScreenAction::WhiteButtonTapped);
    stand.frame_with(ScreenAction::WhiteTemperatureDragged(kelvin(4_200.0)));
    assert_eq!(port.received(), vec![temperature_edit(4_200.0)]);

    port.answer_from_now_on(ACCEPTED);
    stand.frame_with(ScreenAction::ScopeDemandLost);
    assert_eq!(
        port.received(),
        vec![temperature_edit(4_200.0), temperature_edit(4_200.0)],
        "следующий кадр досылает значение сам"
    );
    stand.frame_with(ScreenAction::ScopeDemandLost);
    assert_eq!(port.received().len(), 2, "принято — больше не досылается");
}
