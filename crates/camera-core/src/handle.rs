//! Внешняя ручка движка: команды наружу-внутрь, снимок и события обратно.
//!
//! [`CameraHandle`] дёшево клонируется и безопасен между потоками. Он **ничего не ждёт**:
//! команда кладётся в ограниченную очередь, итоги приходят событиями
//! ([`crate::CameraEvent::CommandHandled`]), а [`CameraHandle::status`] читает уже
//! опубликованный потоком камеры снимок под коротким замком.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use camera_contract::catalog::CameraCatalog;
use camera_contract::control::{CaptureIntent, ManualControl};
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::stream::StreamMode;

use crate::command::{CameraCommand, CommandId, CommandRejected};
use crate::event::CameraEvent;
use crate::inbox::CameraInbox;
use crate::state::{CameraState, ClosedReason};
use crate::status::{AppPresence, CameraCounters, CameraStatus, StreamStats};

struct OutputState {
    status: CameraStatus,
    events: VecDeque<CameraEvent>,
    dropped_events: u64,
}

/// Исходящая сторона: последний снимок и ограниченная очередь событий.
///
/// Снимок и события меняются под одним замком, поэтому наблюдатель, увидевший событие,
/// увидит и снимок не старее него.
pub(crate) struct EngineOutput {
    event_capacity: usize,
    state: Mutex<OutputState>,
    /// Будит ожидающих [`CameraHandle::wait_until_status`] при каждой публикации.
    published: Condvar,
}

impl EngineOutput {
    pub(crate) fn new(presence: AppPresence, event_capacity: usize) -> Self {
        let reason = match presence {
            AppPresence::Foreground => ClosedReason::NotRequested,
            AppPresence::Background => ClosedReason::AppInBackground,
        };
        Self {
            event_capacity,
            state: Mutex::new(OutputState {
                status: CameraStatus {
                    state: CameraState::Closed { reason },
                    presence,
                    catalog: Arc::new(CameraCatalog::empty()),
                    configured: None,
                    stream: StreamStats::empty(),
                    last_unavailable: None,
                    counters: CameraCounters::default(),
                },
                events: VecDeque::new(),
                dropped_events: 0,
            }),
            published: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, OutputState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Опубликовать события и снимок вместе. При переполнении вытесняются старые события
    /// (со счётчиком в `counters.dropped_outward_events`): правду о камере хранит снимок.
    pub(crate) fn publish(&self, mut status: CameraStatus, events: Vec<CameraEvent>) {
        let mut state = self.lock();
        for event in events {
            if state.events.len() >= self.event_capacity {
                state.events.pop_front();
                state.dropped_events += 1;
            }
            state.events.push_back(event);
        }
        status.counters.dropped_outward_events = state.dropped_events;
        state.status = status;
        drop(state);
        self.published.notify_all();
    }

    /// Ждёт снимок, подходящий под условие, но не дольше `timeout`.
    fn wait_for(&self, timeout: Duration, matches: &dyn Fn(&CameraStatus) -> bool) -> Option<CameraStatus> {
        let deadline = Instant::now() + timeout;
        let mut state = self.lock();
        loop {
            if matches(&state.status) {
                return Some(state.status.clone());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            state = self
                .published
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn snapshot(&self) -> CameraStatus {
        self.lock().status.clone()
    }

    fn drain(&self, max: usize) -> Vec<CameraEvent> {
        let mut state = self.lock();
        let take = max.min(state.events.len());
        state.events.drain(..take).collect()
    }
}

/// Ручка к движку камеры (клонируется).
#[derive(Clone)]
pub struct CameraHandle {
    inbox: Arc<CameraInbox>,
    output: Arc<EngineOutput>,
}

impl CameraHandle {
    pub(crate) fn new(inbox: Arc<CameraInbox>, output: Arc<EngineOutput>) -> Self {
        Self { inbox, output }
    }

    fn send(&self, command: CameraCommand) -> Result<CommandId, CommandRejected> {
        self.inbox.submit_command(command)
    }

    /// Хотим эту камеру открытой.
    pub fn open(&self, target: CameraTarget) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::Open { target })
    }

    /// Хотим этот режим потока на желаемой камере.
    pub fn configure(&self, mode: StreamMode) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::Configure { mode })
    }

    pub fn start_streaming(&self, intent: CaptureIntent) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::StartStreaming { intent })
    }

    pub fn stop_streaming(&self) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::StopStreaming)
    }

    /// Перейти на камеру другой роли, сохранив режим и поток.
    pub fn switch_to(&self, role: LensRole) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::SwitchTo { role })
    }

    pub fn apply_controls(&self, controls: ManualControl) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::ApplyControls { controls })
    }

    /// Камера не нужна: закрыть и забыть желаемое.
    pub fn close(&self) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::Close)
    }

    /// Приложение ушло с переднего плана.
    pub fn enter_background(&self) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::EnterBackground)
    }

    /// Приложение вернулось на передний план.
    pub fn enter_foreground(&self) -> Result<CommandId, CommandRejected> {
        self.send(CameraCommand::EnterForeground)
    }

    /// Последний опубликованный снимок; движок не ждёт.
    ///
    /// Счётчик вытесненных частых событий читается из ящика «вживую»: он растёт даже тогда,
    /// когда поток камеры занят и новый снимок ещё не опубликован.
    pub fn status(&self) -> CameraStatus {
        let mut status = self.output.snapshot();
        status.counters.dropped_capture_events = self.inbox.dropped_capture_events();
        status
    }

    /// Блокирует **вызывающего** (не поток камеры) до снимка, подходящего под условие,
    /// но не дольше `timeout`; `None` — не дождались. Для тестов и smoke: UI так не ждёт.
    pub fn wait_until_status(
        &self,
        timeout: Duration,
        matches: impl Fn(&CameraStatus) -> bool,
    ) -> Option<CameraStatus> {
        let mut status = self.output.wait_for(timeout, &matches)?;
        status.counters.dropped_capture_events = self.inbox.dropped_capture_events();
        Some(status)
    }

    /// Забрать до `max` событий в порядке появления.
    pub fn drain_events(&self, max: usize) -> Vec<CameraEvent> {
        self.output.drain(max)
    }
}
