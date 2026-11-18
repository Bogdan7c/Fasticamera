//! Общий запуск сценариев видоискателя (сессия 07): заявить камеру, открыть её в нужном режиме,
//! дождаться `Streaming`, первого показанного кадра и установления автоэкспозиции.
//!
//! Пять сценариев (`viewfinder`, `viewfinder-latency`, `viewfinder-stage-budget`,
//! `viewfinder-endurance`, `video-headroom-bench`) запускаются одинаково и различаются только тем,
//! что делают, когда видоискатель живой. Поэтому запуск — здесь, одним владельцем фаз, а сценарии
//! держат [`ViewfinderSession`] полем и получают от него [`SessionTick`].
//!
//! Камеру сценарий заявляет так же, как `raw-stream`: закрывает продуктовую (автозапуск),
//! открывает свою в нужном режиме; по концу сценария маршрутизатор закрывает её и возвращает
//! видоискатель продукта (`CameraServices::scenario_ended`). Своё открытие — потому что
//! «наш» поток надо отличать от продуктового по поколению сессии, а стенд V4 открывает 16:9.

use std::time::{Duration, Instant};

use camera_core::backend::SessionGeneration;
use camera_core::{CameraEvent, CameraState, CameraStatus};

use super::engine_watch::{RunFailure, failure_of_event, fatal_failure, rejected_failure};
use super::progress::{Deadline, EventLog, TimedNote};
use super::raw_stream_report::DegradedNote;
use super::stream_report::ConfiguredEntry;
use crate::camera_presentation::{CameraPresentation, CameraPresentationObservation};
use crate::camera_scenario_config::{ResolvedStep, SCENARIO_CAPTURE_INTENT};
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{ScenarioContext, stage_fields};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Сроки запуска (из `camera.scenarios.viewfinder`).
#[derive(Debug, Clone, Copy)]
pub struct SessionTiming {
    pub streaming_timeout_ms: u64,
    pub render_live_timeout_ms: u64,
    pub settle_ms: u64,
}

impl SessionTiming {
    pub fn of(config: &ViewfinderScenarioConfig) -> Self {
        Self {
            streaming_timeout_ms: config.streaming_timeout_ms,
            render_live_timeout_ms: config.render_live_timeout_ms,
            settle_ms: config.viewfinder_settle_ms,
        }
    }
}

/// Фаза запуска.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Starting { deadline: Deadline },
    AwaitingRender { deadline: Deadline },
    Settling { until: Instant },
    Live,
}

/// Что сессия сообщает сценарию на тике.
#[derive(Debug)]
pub enum SessionTick {
    /// Ещё запускается.
    Preparing,
    /// Видоискатель только что стал живым (установился): начинать замер. Один раз.
    BecameLive(Box<CameraPresentation>),
    /// Видоискатель живой, поток идёт.
    Live(Box<CameraPresentation>),
    /// Запуск или поток сломались — сценарий кончается провалом.
    Failed(RunFailure),
}

/// Строка стадии, собранная сессией (выводит сценарий: у него `&mut ScenarioContext`).
pub struct SessionStage {
    pub stage: &'static str,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// Итог запуска для отчёта.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SessionSummary {
    pub configured: Option<ConfiguredEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<DegradedNote>,
    pub time_to_streaming_ms: Option<u64>,
    pub time_to_live_ms: Option<u64>,
    /// Состояние камеры на момент сводки.
    pub state_at_end: &'static str,
    /// Последние события камеры (не больше `limit` при сборе).
    pub events: Vec<TimedNote>,
}

/// Запуск и наблюдение за потоком одной камеры сценария.
pub struct ViewfinderSession {
    timing: SessionTiming,
    phase: Phase,
    events: EventLog,
    previous_session: Option<SessionGeneration>,
    presented_at_start: u64,
    configured: Option<ConfiguredEntry>,
    degraded: Option<DegradedNote>,
    time_to_streaming_ms: Option<u64>,
    time_to_live_ms: Option<u64>,
    stages: Vec<SessionStage>,
    /// События 3A (итог тапа, поправки, серия), пришедшие на последнем тике: сессия уже вычерпала очередь движка,
    /// второй вычерпыватель их бы не увидел. Читает сценарий `3a`.
    three_a_events: Vec<CameraEvent>,
}

impl ViewfinderSession {
    /// Заявляет камеру и отправляет команды «закрыть → открыть → режим → поток».
    /// Заявка — ДО первой команды: камеру закроет маршрутизатор при любом конце сценария.
    pub fn start(
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        step: &ResolvedStep,
        timing: SessionTiming,
    ) -> Result<Self, RunFailure> {
        context.camera.claim_camera_for_scenario();
        let previous_session = running.handle().status().stream.session;
        // Хвост событий прошлых сценариев (и автозапуска) не относится к этому прогону.
        running.handle().drain_events(usize::MAX);
        let presented_at_start = presented_now(context).unwrap_or(0);
        send_commands(running, step)?;
        Ok(Self {
            timing,
            phase: Phase::Starting {
                deadline: Deadline::after_millis(context.now, timing.streaming_timeout_ms),
            },
            events: EventLog::started_at(context.now),
            previous_session,
            presented_at_start,
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            stages: Vec::new(),
            three_a_events: Vec::new(),
        })
    }

    /// Стадии, накопленные с прошлого вызова (выводить по порядку).
    pub fn take_stages(&mut self) -> Vec<SessionStage> {
        std::mem::take(&mut self.stages)
    }

    /// События 3A последнего тика (пусто, если их не было). Каждый тик заменяет прежние.
    pub fn three_a_events(&self) -> &[CameraEvent] {
        &self.three_a_events
    }

    /// Один тик: события камеры, фаза запуска, затем живой показ.
    pub fn tick(&mut self, context: &ScenarioContext<'_>, running: &RunningCamera) -> SessionTick {
        let now = context.now;
        let status = running.handle().status();
        let events = running.handle().drain_events(usize::MAX);
        self.events.absorb(now, &events);
        self.note_degradation(&events);
        self.three_a_events = events.iter().filter(|event| is_three_a_event(event)).cloned().collect();
        if let Some(failure) = events
            .iter()
            .find_map(failure_of_event)
            .or_else(|| fatal_failure(&status))
        {
            return SessionTick::Failed(failure);
        }
        match self.phase {
            Phase::Starting { deadline } => self.tick_starting(context, &status, deadline),
            Phase::AwaitingRender { deadline } => self.tick_awaiting_render(context, deadline),
            Phase::Settling { until } => self.tick_settling(context, &status, until),
            Phase::Live => live_presentation(context, &status).map_or_else(SessionTick::Failed, SessionTick::Live),
        }
    }

    fn note_degradation(&mut self, events: &[CameraEvent]) {
        for event in events {
            if let CameraEvent::StreamDegraded {
                requested,
                fallback,
                failure,
            } = event
            {
                self.degraded = Some(DegradedNote {
                    requested_fps: requested.frame_rate.frames_per_second(),
                    fallback_fps: fallback.frame_rate.frames_per_second(),
                    failure: failure.to_string(),
                });
            }
        }
    }

    fn tick_starting(
        &mut self,
        context: &ScenarioContext<'_>,
        status: &CameraStatus,
        deadline: Deadline,
    ) -> SessionTick {
        let now = context.now;
        if let CameraState::Streaming { stream } = &status.state
            && status.stream.session.is_some()
            && status.stream.session != self.previous_session
        {
            let after_ms = self.events.elapsed_ms(now);
            self.configured = Some(ConfiguredEntry::of(stream));
            self.time_to_streaming_ms = Some(after_ms);
            self.stages.push(SessionStage {
                stage: "streaming",
                fields: stage_fields([
                    ("after_ms", after_ms.into()),
                    ("camera", stream.stream.camera.to_string().into()),
                    ("frame_rate", stream.stream.frame_rate.frames_per_second().into()),
                ]),
            });
            // Точка отсчёта показа — момент `Streaming` НАШЕЙ сессии: кадры прежней (продуктовой)
            // сессии, показанные до этого, не считаются «нашим первым кадром».
            self.presented_at_start = presented_now(context).unwrap_or(self.presented_at_start);
            self.phase = Phase::AwaitingRender {
                deadline: Deadline::after_millis(now, self.timing.render_live_timeout_ms),
            };
            return SessionTick::Preparing;
        }
        if deadline.is_expired(now) {
            return SessionTick::Failed(RunFailure::NeverStreaming {
                last_state: status.state.name(),
            });
        }
        SessionTick::Preparing
    }

    fn tick_awaiting_render(&mut self, context: &ScenarioContext<'_>, deadline: Deadline) -> SessionTick {
        let now = context.now;
        let last_seen = match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::RenderUnavailable { reason } => {
                return SessionTick::Failed(RunFailure::RenderUnavailable { reason });
            }
            CameraPresentationObservation::Live(presentation) => {
                if presentation.diagnostics.camera_frames_presented > self.presented_at_start {
                    self.time_to_live_ms = Some(self.events.elapsed_ms(now));
                    self.phase = Phase::Settling {
                        until: now + Duration::from_millis(self.timing.settle_ms),
                    };
                    return SessionTick::Preparing;
                }
                "рендер жив, но кадров камеры ещё не показал"
            }
            CameraPresentationObservation::NotShowingCamera => "рендер не в режиме показа камеры",
        };
        if deadline.is_expired(now) {
            return SessionTick::Failed(RunFailure::RenderNeverShowedCamera { last_seen });
        }
        SessionTick::Preparing
    }

    fn tick_settling(&mut self, context: &ScenarioContext<'_>, status: &CameraStatus, until: Instant) -> SessionTick {
        if context.now < until {
            return SessionTick::Preparing;
        }
        match live_presentation(context, status) {
            Ok(presentation) => {
                self.phase = Phase::Live;
                self.stages.push(SessionStage {
                    // Сигнал скрипту: видоискатель установился (на нём `viewfinder` снимает скриншоты).
                    stage: "viewfinder_live",
                    fields: stage_fields([
                        ("after_ms", self.events.elapsed_ms(context.now).into()),
                        (
                            "presented",
                            presentation
                                .diagnostics
                                .camera_frames_presented
                                .saturating_sub(self.presented_at_start)
                                .into(),
                        ),
                    ]),
                });
                SessionTick::BecameLive(presentation)
            }
            Err(failure) => SessionTick::Failed(failure),
        }
    }

    /// Сводка запуска (для отчёта сценария).
    pub fn summary(&self, running: &RunningCamera, events_limit: usize) -> SessionSummary {
        let notes = self.events.notes();
        let kept = notes.len().min(events_limit);
        SessionSummary {
            configured: self.configured.clone(),
            degraded: self.degraded.clone(),
            time_to_streaming_ms: self.time_to_streaming_ms,
            time_to_live_ms: self.time_to_live_ms,
            state_at_end: running.handle().status().state.name(),
            events: notes[notes.len() - kept..].to_vec(),
        }
    }
}

/// Событие автоматики 3A: итог тапа, поправки ручных значений, итог серии.
fn is_three_a_event(event: &CameraEvent) -> bool {
    matches!(
        event,
        CameraEvent::FocusSettled { .. } | CameraEvent::ControlsAdjusted { .. } | CameraEvent::BracketCaptured { .. }
    )
}

/// Сколько кадров камеры рендер показал к этому моменту (`None` — рендер не показывает камеру).
fn presented_now(context: &ScenarioContext<'_>) -> Option<u64> {
    match context.gpu.observe_camera_presentation() {
        CameraPresentationObservation::Live(presentation) => Some(presentation.diagnostics.camera_frames_presented),
        _ => None,
    }
}

/// Живой показ, пока поток камеры идёт; иначе — причина провала.
fn live_presentation(
    context: &ScenarioContext<'_>,
    status: &CameraStatus,
) -> Result<Box<CameraPresentation>, RunFailure> {
    if !matches!(status.state, CameraState::Streaming { .. }) {
        return Err(RunFailure::StreamInterrupted {
            state: status.state.name(),
        });
    }
    match context.gpu.observe_camera_presentation() {
        CameraPresentationObservation::Live(presentation) => Ok(presentation),
        CameraPresentationObservation::RenderUnavailable { reason } => Err(RunFailure::RenderUnavailable { reason }),
        CameraPresentationObservation::NotShowingCamera => Err(RunFailure::RenderNeverShowedCamera {
            last_seen: "рендер ушёл из режима показа камеры",
        }),
    }
}

/// Команды сценария: чистое состояние, затем открыть, режим и поток.
fn send_commands(running: &RunningCamera, step: &ResolvedStep) -> Result<(), RunFailure> {
    let handle = running.handle();
    handle.close().map_err(|rejected| rejected_failure("close", rejected))?;
    handle
        .open(step.target.clone())
        .map_err(|rejected| rejected_failure("open", rejected))?;
    handle
        .configure(step.mode)
        .map_err(|rejected| rejected_failure("configure", rejected))?;
    handle
        .start_streaming(SCENARIO_CAPTURE_INTENT)
        .map_err(|rejected| rejected_failure("start_streaming", rejected))?;
    Ok(())
}
