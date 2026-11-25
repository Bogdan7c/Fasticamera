//! Фальшивый источник белого для сквозного теста: ОДИН файл, ни одной правки в соседях (движок, рендер, словарь).
//!
//! Шов «новый источник» проверяется самим фактом: этот источник живёт только в тестах и встаёт в реестр одной
//! строкой (`registry.register(fake_source::factory(id, &control))`). Что он ответит на заявку, задаёт тест через
//! [`FakeSourceControl`] — между заявками реакцию можно менять, как менялся бы свет в комнате.

use std::sync::{Arc, Mutex, PoisonError};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::white::{WhiteRequestId, WhiteSourceId, WhiteSourceOutcome};
use camera_contract::white_reading::WhiteReading;
use render_loop::viewfinder::{WhiteSourceBuildError, WhiteSourceRegistry};
use scene_estimation::white_source::{
    WhiteFrame, WhiteInputKind, WhiteReply, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts,
};

/// Что источник делает со следующей заявкой.
#[derive(Debug, Clone, Copy)]
pub enum Reaction {
    /// Ответить `outcome` после того, как увидит `after_frames` кадров с момента заявки.
    Answer {
        outcome: WhiteSourceOutcome,
        after_frames: u32,
    },
    /// Заявку не принять (занят / источника нет).
    Refuse(WhiteRequestRefusal),
    /// Принять и молчать (нейросеть «думает»).
    Silent,
}

/// Что тест знает об источнике и чем им управляет. Клоны делят состояние.
#[derive(Debug)]
pub struct FakeSourceControl {
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    reaction: Reaction,
    requests: Vec<WhiteRequestId>,
    cancels: Vec<WhiteRequestId>,
}

impl FakeSourceControl {
    pub fn new(reaction: Reaction) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                reaction,
                requests: Vec::new(),
                cancels: Vec::new(),
            }),
        })
    }

    /// Новая реакция на следующие заявки.
    pub fn react_with(&self, reaction: Reaction) {
        self.lock().reaction = reaction;
    }

    /// Номера заявок, которые источник получил (в порядке получения).
    pub fn requests(&self) -> Vec<WhiteRequestId> {
        self.lock().requests.clone()
    }

    /// Номера заявок, отменённых владельцем.
    pub fn cancels(&self) -> Vec<WhiteRequestId> {
        self.lock().cancels.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct FakeSource {
    id: WhiteSourceId,
    control: Arc<FakeSourceControl>,
    /// Заявка, на которую ещё не ответили, и сколько кадров прошло с её приёма.
    pending: Option<(WhiteRequestId, u32)>,
    ready: Option<WhiteReply>,
}

impl FakeSource {
    fn answer_if_due(&mut self) {
        let Some((request, frames)) = self.pending else {
            return;
        };
        if let Reaction::Answer { outcome, after_frames } = self.control.lock().reaction
            && frames >= after_frames
        {
            self.pending = None;
            self.ready = Some(WhiteReply {
                request,
                measured_frame: Some(SensorTimestamp::from_nanos(0)),
                outcome,
            });
        }
    }
}

impl WhiteSource for FakeSource {
    fn id(&self) -> WhiteSourceId {
        self.id
    }

    fn required_input(&self) -> WhiteInputKind {
        WhiteInputKind::ColorHints
    }

    fn begin_stream(&mut self, _stream: &WhiteStreamFacts<'_>) {
        self.pending = None;
        self.ready = None;
    }

    fn observe(&mut self, _frame: &WhiteFrame<'_>) {
        if let Some((_, frames)) = &mut self.pending {
            *frames += 1;
        }
        self.answer_if_due();
    }

    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal> {
        let mut state = self.control.lock();
        state.requests.push(request);
        if let Reaction::Refuse(refusal) = state.reaction {
            return Err(refusal);
        }
        drop(state);
        self.pending = Some((request, 0));
        self.answer_if_due();
        Ok(())
    }

    fn cancel(&mut self, request: WhiteRequestId) {
        self.control.lock().cancels.push(request);
        if self.pending.is_some_and(|(pending, _)| pending == request) {
            self.pending = None;
        }
    }

    fn reading(&self) -> WhiteReading {
        // Сквозной тест про заявки и ответы; показание фальшивый источник не ведёт.
        WhiteReading::NotMeasuredYet
    }

    fn poll(&mut self) -> Option<WhiteReply> {
        self.ready.take()
    }
}

/// Фабрика источника для реестра: `registry.register(fake_source::factory(id, &control))`.
pub fn factory(
    id: WhiteSourceId,
    control: &Arc<FakeSourceControl>,
) -> impl Fn() -> Result<Box<dyn WhiteSource>, WhiteSourceBuildError> + Send + Sync + 'static {
    let control = Arc::clone(control);
    move || {
        Ok(Box::new(FakeSource {
            id,
            control: Arc::clone(&control),
            pending: None,
            ready: None,
        }) as Box<dyn WhiteSource>)
    }
}

/// Реестр из одного фальшивого источника.
pub fn registry_with(id: WhiteSourceId, control: &Arc<FakeSourceControl>) -> WhiteSourceRegistry {
    let mut registry = WhiteSourceRegistry::new();
    registry.register(factory(id, control));
    registry
}
