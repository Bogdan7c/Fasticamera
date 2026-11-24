//! «Владелец белого» для GPU-тестов рендера: роль движка камеры играет сам тест.
//!
//! Движок камеры (`camera-core`) живёт в другом крейте и рендеру недоступен. Рендеру от него нужны только
//! две вещи: публиковать указания в `WhiteLinks::directives` и читать ответы источников из
//! `WhiteLinks::answers`. Этот помощник делает ровно это по правилам плана: открытие — заявка источнику с
//! `HideFrames`; готовый ответ по ДЕЙСТВУЮЩЕЙ заявке принимается плавно; отказ белую не меняет; ответ на чужой
//! номер (рука, отмена, сменившаяся заявка) игнорируется.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть помощников")]

use std::time::Duration;

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::three_a::DirectiveGeneration;
use camera_contract::white::{
    SceneIlluminant, WhileAwaitingWhite, WhiteAnswer, WhiteDirective, WhiteRequestId, WhiteSourceId,
    WhiteSourceOutcome, WhiteSourceRequest, WhiteTransition,
};
use render_loop::viewfinder::WhiteLinks;

/// Как быстро принимается поздний ответ (WB23).
pub const LATE_ANSWER_TRANSITION: Duration = Duration::from_millis(300);

/// Белая по температуре, нулевой оттенок.
pub fn setting_of(kelvin: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::NEUTRAL,
    }
}

/// Тестовый движок: хранит текущую белую и номер заявки, как владелец белого.
pub struct WhiteTestEngine {
    links: WhiteLinks,
    seen: Option<DirectiveGeneration>,
    current: WhiteBalanceSetting,
    request: Option<WhiteSourceRequest>,
    next_request_id: u64,
    /// Все ответы источников, которые движок прочитал (в порядке публикации).
    pub answers_read: Vec<WhiteAnswer>,
    /// Ответов, принятых движком (белая сменилась по ответу).
    pub answers_accepted: u64,
}

impl WhiteTestEngine {
    /// Открытие камеры: белая-запасная (дневной свет из конфига) и заявка источнику `source` с `HideFrames`.
    pub fn open(links: &WhiteLinks, fallback_kelvin: f64, source: WhiteSourceId) -> Self {
        let mut engine = Self {
            links: links.clone(),
            seen: None,
            current: setting_of(fallback_kelvin),
            request: None,
            next_request_id: 1,
            answers_read: Vec::new(),
            answers_accepted: 0,
        };
        engine.ask(source, WhileAwaitingWhite::HideFrames);
        engine
    }

    /// Номер действующей заявки.
    pub fn request_id(&self) -> Option<WhiteRequestId> {
        self.request.map(|request| request.id)
    }

    /// Белая, которую держит движок.
    pub fn current(&self) -> WhiteBalanceSetting {
        self.current
    }

    fn publish(&self, transition: WhiteTransition) {
        self.links.directives.publish(WhiteDirective {
            white: self.current,
            transition,
            request: self.request,
        });
    }

    /// Новая заявка источнику (кнопка «Телефон» или открытие).
    pub fn ask(&mut self, source: WhiteSourceId, while_awaiting: WhileAwaitingWhite) {
        let id = WhiteRequestId(self.next_request_id);
        self.next_request_id += 1;
        self.request = Some(WhiteSourceRequest {
            id,
            source,
            while_awaiting,
        });
        self.publish(WhiteTransition::Immediate);
    }

    /// Ручная команда: белая задана, заявка отменена (номер больше не действует).
    pub fn hold(&mut self, white: WhiteBalanceSetting, transition: WhiteTransition) {
        self.current = white;
        self.request = None;
        self.publish(transition);
    }

    /// Читает ящик ответов (зовётся на каждом кадре стенда) и применяет то, что положено.
    pub fn react(&mut self) {
        let Some((generation, answer)) = self.links.answers.newer_than(self.seen) else {
            return;
        };
        self.seen = Some(generation);
        self.answers_read.push(answer);
        if self.request_id() != Some(answer.request) {
            // Ответ на чужую заявку: рука или смена заявки сильнее.
            return;
        }
        match answer.outcome {
            WhiteSourceOutcome::Ready {
                illuminant: SceneIlluminant::Uniform(white),
                ..
            } => {
                self.current = white;
                self.request = None;
                self.answers_accepted += 1;
                self.publish(WhiteTransition::Over(LATE_ANSWER_TRANSITION));
            }
            // Отказ, «не знаю», «занят», «сломался»: белая прежняя, заявка закрыта.
            _ => {
                self.request = None;
                self.publish(WhiteTransition::Immediate);
            }
        }
    }
}

// ---- фальшивый источник белого ----

use std::sync::{Arc, Mutex, PoisonError};

use camera_contract::capture_result::SensorTimestamp;
use render_loop::viewfinder::WhiteSourceRegistry;
use scene_estimation::white_source::{
    WhiteFrame, WhiteInputKind, WhiteReply, WhiteRequestRefusal, WhiteSource, WhiteStreamFacts,
};

/// Имя фальшивого источника.
pub const SCRIPTED_ID: WhiteSourceId = WhiteSourceId::new("scripted");

/// Что фальшивый источник делает с заявкой.
#[derive(Debug, Clone, Copy)]
pub enum Script {
    /// Заявку не принимает.
    Refuse(WhiteRequestRefusal),
    /// Отвечает `outcome` после того, как увидит `observed_frames` кадров потока.
    AnswerAfter {
        observed_frames: u32,
        outcome: WhiteSourceOutcome,
    },
    /// Принимает и молчит.
    Silent,
}

/// Журнал вызовов фальшивого источника.
#[derive(Debug, Default)]
pub struct ScriptedLog {
    pub requests: Vec<WhiteRequestId>,
    pub cancels: Vec<WhiteRequestId>,
    pub streams_begun: u32,
    pub frames_observed: u32,
}

struct ScriptedSource {
    script: Script,
    input: WhiteInputKind,
    log: Arc<Mutex<ScriptedLog>>,
    observed_in_stream: u32,
    pending: Option<WhiteRequestId>,
    ready: Option<WhiteReply>,
}

impl ScriptedSource {
    fn answer_if_due(&mut self) {
        let (
            Some(request),
            Script::AnswerAfter {
                observed_frames,
                outcome,
            },
        ) = (self.pending, self.script)
        else {
            return;
        };
        if self.observed_in_stream >= observed_frames {
            self.pending = None;
            self.ready = Some(WhiteReply {
                request,
                measured_frame: Some(SensorTimestamp::from_nanos(0)),
                outcome,
            });
        }
    }
}

impl WhiteSource for ScriptedSource {
    fn id(&self) -> WhiteSourceId {
        SCRIPTED_ID
    }

    fn required_input(&self) -> WhiteInputKind {
        self.input
    }

    fn begin_stream(&mut self, _stream: &WhiteStreamFacts<'_>) {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).streams_begun += 1;
        self.observed_in_stream = 0;
        self.pending = None;
        self.ready = None;
    }

    fn observe(&mut self, _frame: &WhiteFrame<'_>) {
        self.log.lock().unwrap_or_else(PoisonError::into_inner).frames_observed += 1;
        self.observed_in_stream += 1;
        self.answer_if_due();
    }

    fn request(&mut self, request: WhiteRequestId) -> Result<(), WhiteRequestRefusal> {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .requests
            .push(request);
        if let Script::Refuse(refusal) = self.script {
            return Err(refusal);
        }
        self.pending = Some(request);
        self.answer_if_due();
        Ok(())
    }

    fn cancel(&mut self, request: WhiteRequestId) {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancels
            .push(request);
        if self.pending == Some(request) {
            self.pending = None;
        }
    }

    fn poll(&mut self) -> Option<WhiteReply> {
        self.ready.take()
    }
}

/// Реестр с одним сценарным источником и его журналом.
pub fn scripted_registry(script: Script, input: WhiteInputKind) -> (WhiteSourceRegistry, Arc<Mutex<ScriptedLog>>) {
    let log = Arc::new(Mutex::new(ScriptedLog::default()));
    let mut registry = WhiteSourceRegistry::new();
    let shared = Arc::clone(&log);
    registry.register(move || {
        Ok(Box::new(ScriptedSource {
            script,
            input,
            log: Arc::clone(&shared),
            observed_in_stream: 0,
            pending: None,
            ready: None,
        }) as Box<dyn WhiteSource>)
    });
    (registry, log)
}

/// Готовый ответ «свет такой-то» для сценария.
pub fn lamp_outcome(kelvin: f64) -> WhiteSourceOutcome {
    WhiteSourceOutcome::Ready {
        illuminant: SceneIlluminant::Uniform(setting_of(kelvin)),
        confidence: camera_contract::white::WhiteConfidence::FULL,
    }
}

/// Связка, у которой белая названа заранее и держится: указание сразу, заявок нет (эталонный прогон).
pub fn held_setting(setting: WhiteBalanceSetting) -> WhiteLinks {
    let links = WhiteLinks::default();
    links.directives.publish(WhiteDirective {
        white: setting,
        transition: WhiteTransition::Immediate,
        request: None,
    });
    links
}
