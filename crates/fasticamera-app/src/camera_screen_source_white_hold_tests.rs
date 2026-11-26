//! Пауза слежения за белым на время раздела калибровки через настоящий источник кадров (решение 289): экран отвечает
//! на запрос «чего хочет режим», источник каждый кадр сверяет это с принятым движком и досылает разницу, пока она не
//! принята. Проверяем то, что видит движок: какие уровни и сколько раз до него дошли.

use camera_screen::modes::CaptureMode;
use camera_screen::{ScreenAction, WhiteTrackingHold};
use shell_contract::orientation::DeviceOrientation;

use super::*;
use crate::camera_screen_input::ScreenInputSender;

/// Порт камеры, который помнит только уровни паузы белого и отвечает на них заданным итогом.
struct HoldPort {
    outcome: Arc<Mutex<CameraIntentOutcome>>,
    holds: Arc<Mutex<Vec<WhiteTrackingHold>>>,
}

impl CameraIntentPort for HoldPort {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        if let ScreenCameraIntent::HoldWhiteTracking(hold) = intent {
            self.holds.lock().expect("замок").push(hold);
            return *self.outcome.lock().expect("замок");
        }
        CameraIntentOutcome::Sent(CommandId(0))
    }
}

struct Stand {
    source: CameraScreenSource,
    sender: ScreenInputSender,
    board: Arc<ScreenBoard>,
    outcome: Arc<Mutex<CameraIntentOutcome>>,
    holds: Arc<Mutex<Vec<WhiteTrackingHold>>>,
    now_ms: i64,
}

impl Stand {
    /// Экран в режиме PHOTO, порт принимает всё.
    fn start() -> Self {
        let outcome = Arc::new(Mutex::new(CameraIntentOutcome::Sent(CommandId(1))));
        let holds = Arc::new(Mutex::new(Vec::new()));
        let (sender, receiver) = screen_input_channel(16);
        let board = Arc::new(ScreenBoard::default());
        let source = CameraScreenSource::new(ScreenSourceParts {
            look_white_stops: crate::look_white_stops::LookWhiteStopsWatcher::silent(),
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
                camera: Box::new(HoldPort {
                    outcome: Arc::clone(&outcome),
                    holds: Arc::clone(&holds),
                }),
                haptics: Arc::new(QuietHaptics),
                commands: Arc::new(QuietCommands),
                render: RecordingRenderControl::new(),
                capture: Arc::new(NoCapture),
                shutter_sound: Arc::new(crate::camera_screen_service::SilentShutterSound::default()),
                lens: camera_contract::zoom::LensIntentChannel::new(),
                look: None,
            }),
            board: Arc::clone(&board),
            report_min_interval: Duration::ZERO,
        });
        Self {
            source,
            sender,
            board,
            outcome,
            holds,
            now_ms: 0,
        }
    }

    /// `count` кадров UI подряд.
    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.now_ms += 16;
            self.source.produce_frame(&frame_request(self.now_ms)).expect("кадр");
        }
    }

    fn act(&mut self, action: ScreenAction) {
        self.sender.send_action(action);
        self.frames(1);
    }

    fn answer(&self, outcome: CameraIntentOutcome) {
        *self.outcome.lock().expect("замок") = outcome;
    }

    /// Какие уровни паузы доходили до порта, по порядку (включая отказанные попытки).
    fn attempts(&self) -> Vec<WhiteTrackingHold> {
        self.holds.lock().expect("замок").clone()
    }

    fn counters(&self) -> crate::camera_white_hold::WhiteHoldCounters {
        self.board.latest().expect("отчёт").effects.white_hold
    }
}

const RUNNING: WhiteTrackingHold = WhiteTrackingHold::Running;
const PAUSED: WhiteTrackingHold = WhiteTrackingHold::PausedForCalibration;

#[test]
fn a_process_starting_in_photo_tells_the_engine_running_once_and_then_stays_quiet() {
    let mut stand = Stand::start();
    stand.frames(6);
    // Принятое вначале «неизвестно», поэтому первый кадр сообщает желание (движок мог остаться на паузе от прежнего
    // экрана); дальше — тишина, а не команда на каждом кадре.
    assert_eq!(stand.attempts(), vec![RUNNING]);
    assert_eq!(stand.counters().accepted, 1);
}

#[test]
fn entering_and_leaving_calibrate_each_send_one_command_and_rotation_or_camera_loss_change_nothing() {
    let mut stand = Stand::start();
    stand.frames(2);

    stand.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    stand.frames(3);
    assert_eq!(stand.attempts(), vec![RUNNING, PAUSED], "вход в раздел — пауза");

    // Поворот телефона, пропажа и возвращение камеры (уход в фон и обратно для экрана — это факты камеры) режим не
    // меняют, а значит, и желание: команд больше нет.
    stand.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    stand.act(ScreenAction::CameraFactsChanged(CameraFacts::STARTING));
    stand.frames(2);
    stand.act(ScreenAction::CameraFactsChanged(CameraFacts {
        presence: CameraPresence::Live,
        ..CameraFacts::STARTING
    }));
    stand.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::Portrait));
    stand.frames(3);
    assert_eq!(
        stand.attempts(),
        vec![RUNNING, PAUSED],
        "поворот и потеря камеры паузу не трогают"
    );

    stand.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    stand.frames(3);
    assert_eq!(
        stand.attempts(),
        vec![RUNNING, PAUSED, RUNNING],
        "выход из раздела — снять паузу"
    );
}

#[test]
fn a_refused_pause_is_resent_every_frame_until_the_engine_takes_it() {
    let mut stand = Stand::start();
    stand.frames(2);
    for refusal in [
        CameraIntentOutcome::QueueFull,
        CameraIntentOutcome::EngineStopped,
        CameraIntentOutcome::NoEngine,
    ] {
        stand.answer(refusal);
        let before = stand.attempts().len();
        stand.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
        stand.frames(3);
        let tried = stand.attempts()[before..].to_vec();
        assert_eq!(tried, vec![PAUSED; 4], "{refusal:?}: пауза повторяется на каждом кадре");
        // Движок вернулся: следующий кадр принимает, и дальше тишина.
        stand.answer(CameraIntentOutcome::Sent(CommandId(9)));
        stand.frames(1);
        stand.frames(4);
        assert_eq!(
            stand.attempts().len(),
            before + 5,
            "{refusal:?}: принятое не досылается"
        );
        assert_eq!(stand.attempts().last(), Some(&PAUSED));
        // Вернуться в PHOTO к следующему кругу.
        stand.act(ScreenAction::ModeSelected(CaptureMode::Photo));
        stand.frames(2);
    }
    let counters = stand.counters();
    assert_eq!(
        (counters.queue_full, counters.engine_stopped, counters.no_engine),
        (4, 4, 4)
    );
}

#[test]
fn a_lost_unpause_does_not_freeze_the_white_the_next_frames_keep_trying() {
    let mut stand = Stand::start();
    stand.frames(2);
    stand.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    stand.frames(2);
    assert_eq!(stand.attempts(), vec![RUNNING, PAUSED]);

    // Выход из раздела при полной очереди движка: команда «снять паузу» потеряна, но желание живо.
    stand.answer(CameraIntentOutcome::QueueFull);
    stand.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    stand.frames(4);
    let tail: Vec<_> = stand.attempts()[2..].to_vec();
    assert_eq!(tail, vec![RUNNING; 5], "пауза не забыта: попытка на каждом кадре");

    stand.answer(CameraIntentOutcome::Sent(CommandId(3)));
    stand.frames(3);
    assert_eq!(stand.attempts().len(), 8, "принята одной попыткой, дальше тишина");
}
