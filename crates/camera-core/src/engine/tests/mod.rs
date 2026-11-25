//! Тесты движка на fake-бэкенде. Время и события управляются вручную: `Rig` — «стенд»,
//! который гоняет события fake в движок и ведёт журнал событий движка.

mod commands;
mod flash_decisions;
mod flash_led_white;
mod flash_preflash;
mod flash_sequence;
mod flash_support;
mod frame_rate_fallback;
mod lens_blocks;
mod lens_frame_rate;
mod lens_intents;
mod lens_support;
mod lens_switching;
mod lifecycle;
mod recovery;
mod recovery_limits;
mod standby_demand;
mod standby_failures;
mod standby_promotion;
mod standby_support;
mod stream_start_exposure;
mod three_a_bracket;
mod three_a_controllers;
mod three_a_controls;
mod three_a_focus;
mod three_a_hal_ae_counter;
mod three_a_own;
mod three_a_support;
mod transitions;
mod white_adjust;
mod white_answers;
mod white_journal;
mod white_manual;
mod white_open;
mod white_resume_rule;
mod white_support;
mod white_take_report;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};

use super::CameraEngine;
use crate::backend::{BackendEvent, BackendEventListener, BackendEventSink};
use crate::command::{CameraCommand, CommandId, CommandOutcome};
use crate::config::CameraEngineConfig;
use crate::event::CameraEvent;
use crate::fake::{BackendCall, FakeBackendControl};
use crate::state::{CameraError, CameraState, ClosedReason, FatalCause};
use crate::status::AppPresence;
use crate::three_a_wiring::ThreeAWiring;

/// Собирает события fake, чтобы стенд отдавал их движку в нужный момент.
#[derive(Default)]
struct Collector {
    events: Mutex<Vec<BackendEvent>>,
}

impl BackendEventListener for Collector {
    fn deliver(&self, event: BackendEvent) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

pub(super) struct Rig {
    pub(super) engine: CameraEngine,
    pub(super) fake: Arc<FakeBackendControl>,
    collector: Arc<Collector>,
    pub(super) now: Instant,
    next_command: u64,
    log: Vec<CameraEvent>,
    cursor: usize,
}

pub(super) fn mode_raw10_60() -> StreamMode {
    StreamMode {
        raw_format: RawFormat::Raw10,
        readout: ReadoutAspect::SixteenByNine,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
    }
}

pub(super) fn mode_raw16_30() -> StreamMode {
    StreamMode {
        raw_format: RawFormat::Raw16,
        readout: ReadoutAspect::FourByThree,
        frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
    }
}

pub(super) fn wide() -> CameraTarget {
    CameraTarget::Role(LensRole::Wide)
}

impl Rig {
    /// Стенд с каталогом OnePlus 11, приложение на переднем плане, каталог прочитан.
    pub(super) fn new() -> Self {
        Self::with_config(CameraEngineConfig::default(), AppPresence::Foreground)
    }

    pub(super) fn with_config(config: CameraEngineConfig, presence: AppPresence) -> Self {
        let fake = FakeBackendControl::new();
        let rig = Self::over_fake(config, presence, fake);
        let mut rig = rig;
        rig.engine.load_catalog(rig.now);
        rig.settle();
        rig.fake.take_calls();
        rig
    }

    /// Стенд над готовым fake, но каталог ещё не прочитан.
    pub(super) fn over_fake(config: CameraEngineConfig, presence: AppPresence, fake: Arc<FakeBackendControl>) -> Self {
        let wiring = ThreeAWiring::vendor(&config.three_a);
        Self::over_fake_with_wiring(config, presence, fake, wiring)
    }

    /// Стенд с заданными исполнителями 3A и ящиком баланса белого; каталог прочитан.
    pub(super) fn with_wiring(config: CameraEngineConfig, wiring: ThreeAWiring) -> Self {
        let fake = FakeBackendControl::new();
        let mut rig = Self::over_fake_with_wiring(config, AppPresence::Foreground, fake, wiring);
        rig.engine.load_catalog(rig.now);
        rig.settle();
        rig.fake.take_calls();
        rig
    }

    fn over_fake_with_wiring(
        config: CameraEngineConfig,
        presence: AppPresence,
        fake: Arc<FakeBackendControl>,
        wiring: ThreeAWiring,
    ) -> Self {
        let collector = Arc::new(Collector::default());
        let listener: Arc<dyn BackendEventListener> = Arc::clone(&collector) as Arc<dyn BackendEventListener>;
        let backend = fake.backend(BackendEventSink::new(listener));
        Self {
            engine: CameraEngine::with_wiring(config, presence, backend, wiring),
            fake,
            collector,
            now: Instant::now(),
            next_command: 0,
            log: Vec::new(),
            cursor: 0,
        }
    }

    /// Стенд из готовых частей (движок собран тестом, например с ящиком намерений объектива).
    fn from_parts(engine: CameraEngine, fake: Arc<FakeBackendControl>, collector: Arc<Collector>) -> Self {
        Self {
            engine,
            fake,
            collector,
            now: Instant::now(),
            next_command: 0,
            log: Vec::new(),
            cursor: 0,
        }
    }

    /// Отдать движку все события, которые fake успел прислать.
    pub(super) fn pump(&mut self) {
        loop {
            let batch: Vec<BackendEvent> =
                std::mem::take(&mut *self.collector.events.lock().unwrap_or_else(PoisonError::into_inner));
            if batch.is_empty() {
                break;
            }
            for event in batch {
                self.engine.handle_backend_event(event, self.now);
            }
        }
        self.settle();
    }

    fn settle(&mut self) {
        self.log.extend(self.engine.drain_events());
    }

    /// Команда, ответ на неё и последствия (события fake тоже обработаны).
    pub(super) fn send(&mut self, command: CameraCommand) -> CommandOutcome {
        self.next_command += 1;
        let id = CommandId(self.next_command);
        self.engine.handle_command(id, command, self.now);
        self.pump();
        let answers: Vec<&CommandOutcome> = self.log.iter().filter_map(|event| event.command_outcome(id)).collect();
        assert_eq!(answers.len(), 1, "на команду должен быть ровно один итог");
        answers[0].clone()
    }

    /// Номер последней отправленной команды (он же номер тапа).
    pub(super) fn last_command_id(&self) -> CommandId {
        CommandId(self.next_command)
    }

    /// Событие бэкенда напрямую в движок (в том числе с чужим поколением).
    pub(super) fn feed(&mut self, event: BackendEvent) {
        self.engine.handle_backend_event(event, self.now);
        self.pump();
    }

    /// Прокрутить часы и дать таймеру сработать.
    pub(super) fn advance(&mut self, by: Duration) {
        self.now += by;
        self.engine.on_timer(self.now);
        self.pump();
    }

    /// События движка с прошлого вызова.
    pub(super) fn fresh_events(&mut self) -> Vec<CameraEvent> {
        self.settle();
        let fresh = self.log[self.cursor..].to_vec();
        self.cursor = self.log.len();
        fresh
    }

    /// Только смены состояния с прошлого вызова — именами.
    pub(super) fn fresh_state_names(&mut self) -> Vec<&'static str> {
        state_names(&self.fresh_events())
    }

    pub(super) fn state(&self) -> CameraState {
        self.engine.status().state
    }

    pub(super) fn state_name(&self) -> &'static str {
        self.state().name()
    }

    // ---- Короткие сценарии ----

    /// Довести до `Streaming` на основной камере.
    pub(super) fn start_streaming_on_wide(&mut self) {
        self.start_streaming_on(wide());
    }

    /// Довести до `Streaming` на выбранной камере.
    pub(super) fn start_streaming_on(&mut self, target: CameraTarget) {
        assert_eq!(self.send(CameraCommand::Open { target }), CommandOutcome::Accepted);
        assert_eq!(
            self.send(CameraCommand::Configure { mode: mode_raw10_60() }),
            CommandOutcome::Accepted
        );
        assert_eq!(
            self.send(CameraCommand::StartStreaming {
                intent: CaptureIntent::Preview
            }),
            CommandOutcome::Accepted
        );
        assert_eq!(self.state_name(), "streaming");
        self.fresh_events();
        self.fake.take_calls();
    }
}

pub(super) fn state_names(events: &[CameraEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|event| match event {
            CameraEvent::StateChanged { state } => Some(state.name()),
            _ => None,
        })
        .collect()
}

/// Журнал вызовов без аргументов — для проверки порядка.
pub(super) fn call_names(calls: &[BackendCall]) -> Vec<&'static str> {
    calls
        .iter()
        .map(|call| match call {
            BackendCall::LoadCatalog => "load_catalog",
            BackendCall::OpenDevice { .. } => "open_device",
            BackendCall::ConfigureSession { .. } => "configure_session",
            BackendCall::SetRepeatingRequest { .. } => "set_repeating_request",
            BackendCall::SubmitFocusTrigger { .. } => "submit_focus_trigger",
            BackendCall::SubmitBracket { .. } => "submit_bracket",
            BackendCall::SubmitFlashStill { .. } => "submit_flash_still",
            BackendCall::StopRepeating => "stop_repeating",
            BackendCall::CloseSession => "close_session",
            BackendCall::CloseDevice => "close_device",
            BackendCall::OpenStandbyDevice { .. } => "open_standby_device",
            BackendCall::ConfigureStandbySession { .. } => "configure_standby_session",
            BackendCall::SetStandbyRepeatingRequest { .. } => "set_standby_repeating_request",
            BackendCall::CloseStandby { .. } => "close_standby",
            BackendCall::SwapStandbyWithPrimary { .. } => "swap_standby_with_primary",
        })
        .collect()
}

/// Основная камера, 4:3, заданная частота — режим фото-видоискателя (решение 23).
pub(super) fn mode_raw10_4x3_at(fps: u32) -> StreamMode {
    StreamMode {
        raw_format: RawFormat::Raw10,
        readout: ReadoutAspect::FourByThree,
        frame_rate: TargetFrameRate::new(fps).expect("валидная частота"),
    }
}
