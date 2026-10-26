//! Сценарий `camera-evict` (только `debug-scenarios`): вытеснение камеры системной и самовосстановление.
//!
//! Ход прогона (решение 39, поведение под ответственностью движка):
//! 1. сценарий включает `HoldInBackgroundForDiagnostics` (камера не отпускается в фоне) и заявляет права
//!    на камеру, открывает роль `Wide` в RAW10 4:3@30 и ждёт `Streaming`; пишет стадию `streaming`;
//! 2. скрипт на стадии `streaming` запускает системную камеру, ждёт и возвращает приложение;
//! 3. система отнимает у нас камеру — сценарий ждёт `Unavailable{..}` (причина попадает в отчёт);
//! 4. после возврата приложения (`Resumed` → `enter_foreground`) движок сам восстанавливает поток —
//!    сценарий ждёт `Recovered{..}` и снова `Streaming`.
//!
//! Политику по умолчанию возвращает и камеру закрывает маршрутизатор при любом конце сценария;
//! сценарий сам этого не делает, поэтому не может «забыть» на каком-нибудь пути.

use std::time::Instant;

use camera_core::backend::SessionGeneration;
use camera_core::{CameraEvent, CameraState, CameraStatus};

use super::detail::EVENTS_IN_REPORT_LIMIT;
use super::engine_watch::{RunFailure, failure_of_event, fatal_failure, rejected_failure, unavailable_reasons};
use super::evict_report::{EvictObservation, RecoveredSeen, UnavailableSeen, evaluate_evict};
use super::progress::{Deadline, EventLog};
use crate::camera_scenario_config::{EvictScenarioConfig, ResolvedStep, SCENARIO_CAPTURE_INTENT};
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, judged, stage_fields,
};

#[derive(Debug, Clone, Copy)]
enum Phase {
    /// Ждём первый `Streaming` (до вытеснения).
    Streaming { deadline: Deadline },
    /// Поток идёт, скрипт запускает системную камеру; ждём `Unavailable`.
    Eviction { deadline: Deadline },
    /// Камера отнята, ждём самовосстановление.
    Recovery { deadline: Deadline },
}

/// Куда двигается прогон после тика.
enum Progress {
    Continue,
    /// Показать скрипту стадию `streaming`: поток идёт, можно запускать системную камеру.
    StreamingReached,
    Finished,
}

/// Состояние прогона.
struct Run {
    phase: Phase,
    events: EventLog,
    config: EvictScenarioConfig,
    /// Сессия до открытия: «наш» первый `Streaming` — уже с другой сессией.
    previous_session: Option<SessionGeneration>,
    observation: EvictObservation,
}

#[derive(Default)]
pub struct CameraEvictScenario {
    run: Option<Run>,
}

impl CameraEvictScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Run {
    fn new(config: EvictScenarioConfig, now: Instant, previous_session: Option<SessionGeneration>) -> Self {
        let phase = Phase::Streaming {
            deadline: Deadline::after_millis(now, config.streaming_timeout_ms),
        };
        Self {
            phase,
            events: EventLog::started_at(now),
            config,
            previous_session,
            observation: EvictObservation {
                failure: None,
                camera: None,
                streaming_after_ms: None,
                unavailable: None,
                further_unavailable: Vec::new(),
                recovered: None,
                final_state: "unknown",
                counters: camera_core::CameraCounters::default(),
                events: Vec::new(),
                events_dropped: 0,
            },
        }
    }

    fn tick(&mut self, running: &RunningCamera, now: Instant) -> Progress {
        let status = running.handle().status();
        let events = running.handle().drain_events(usize::MAX);
        self.events.absorb(now, &events);
        self.observation.final_state = status.state.name();
        self.observation.counters = status.counters;
        if let Some(failure) = events
            .iter()
            .find_map(failure_of_event)
            .or_else(|| fatal_failure(&status))
        {
            self.observation.failure = Some(failure);
            return Progress::Finished;
        }
        self.note_unavailable(&events, now);
        match self.phase {
            Phase::Streaming { deadline } => self.tick_streaming(&status, now, deadline),
            Phase::Eviction { deadline } => self.tick_eviction(&status, now, deadline),
            Phase::Recovery { deadline } => self.tick_recovery(&status, &events, now, deadline),
        }
    }

    /// Первая недоступность после того, как поток шёл, — искомое вытеснение; остальные — для разбора.
    fn note_unavailable(&mut self, events: &[CameraEvent], now: Instant) {
        for reason in unavailable_reasons(events) {
            let is_the_eviction = matches!(self.phase, Phase::Eviction { .. });
            if is_the_eviction {
                self.observation.unavailable = Some(UnavailableSeen {
                    reason,
                    at_ms: self.events.elapsed_ms(now),
                });
                self.phase = Phase::Recovery {
                    deadline: Deadline::after_millis(now, self.config.recovery_timeout_ms),
                };
            } else {
                self.observation.further_unavailable.push(reason);
            }
        }
    }

    fn tick_streaming(&mut self, status: &CameraStatus, now: Instant, deadline: Deadline) -> Progress {
        if let CameraState::Streaming { stream } = &status.state
            && status.stream.session.is_some()
            && status.stream.session != self.previous_session
        {
            self.observation.camera = Some(stream.stream.camera.to_string());
            self.observation.streaming_after_ms = Some(self.events.elapsed_ms(now));
            self.phase = Phase::Eviction {
                deadline: Deadline::after_millis(now, self.config.eviction_timeout_ms),
            };
            return Progress::StreamingReached;
        }
        if deadline.is_expired(now) {
            self.observation.failure = Some(RunFailure::NeverStreaming {
                last_state: status.state.name(),
            });
            return Progress::Finished;
        }
        Progress::Continue
    }

    fn tick_eviction(&mut self, status: &CameraStatus, now: Instant, deadline: Deadline) -> Progress {
        if deadline.is_expired(now) {
            self.observation.failure = Some(RunFailure::NeverEvicted {
                last_state: status.state.name(),
            });
            return Progress::Finished;
        }
        Progress::Continue
    }

    fn tick_recovery(
        &mut self,
        status: &CameraStatus,
        events: &[CameraEvent],
        now: Instant,
        deadline: Deadline,
    ) -> Progress {
        let recovered = events.iter().find_map(|event| match event {
            CameraEvent::Recovered { reason, attempts } => Some((*reason, *attempts)),
            _ => None,
        });
        if let Some((reason, attempts)) = recovered {
            let at_ms = self.events.elapsed_ms(now);
            let evicted_at_ms = self.observation.unavailable.as_ref().map_or(0, |seen| seen.at_ms);
            self.observation.recovered = Some(RecoveredSeen {
                reason,
                attempts,
                at_ms,
                recovery_ms: at_ms.saturating_sub(evicted_at_ms),
            });
        }
        if self.observation.recovered.is_some() && matches!(status.state, CameraState::Streaming { .. }) {
            return Progress::Finished;
        }
        if deadline.is_expired(now) {
            self.observation.failure = Some(RunFailure::NeverRecovered {
                last_state: status.state.name(),
            });
            return Progress::Finished;
        }
        Progress::Continue
    }

    /// Итог прогона: наблюдения и вердикт.
    fn into_outcome(mut self) -> crate::scenario::ScenarioOutcome {
        let notes = self.events.notes();
        let kept = notes.len().min(EVENTS_IN_REPORT_LIMIT);
        self.observation.events = notes[..kept].to_vec();
        self.observation.events_dropped = notes.len() - kept;
        let violations = evaluate_evict(&self.observation);
        judged(self.observation, violations)
    }
}

/// Команды сценария: чистое состояние, затем открыть, режим и поток.
fn send_commands(running: &RunningCamera, resolved: &ResolvedStep) -> Result<(), RunFailure> {
    let handle = running.handle();
    handle.close().map_err(|rejected| rejected_failure("close", rejected))?;
    handle
        .open(resolved.target.clone())
        .map_err(|rejected| rejected_failure("open", rejected))?;
    handle
        .configure(resolved.mode)
        .map_err(|rejected| rejected_failure("configure", rejected))?;
    handle
        .start_streaming(SCENARIO_CAPTURE_INTENT)
        .map_err(|rejected| rejected_failure("start_streaming", rejected))?;
    Ok(())
}

impl ObservingScenario for CameraEvictScenario {
    fn name(&self) -> &'static str {
        "camera-evict"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let running = match context.camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = context.camera.scenarios().evict.clone();
        let resolved = match config.resolve() {
            Ok(resolved) => resolved,
            Err(error) => return ScenarioStep::Finished(failed(format!("настройки camera.scenarios.evict: {error}"))),
        };
        // Удержание и заявка — ДО первой команды: политику вернёт и камеру закроет маршрутизатор
        // при любом конце сценария, даже если следующая строка вернёт ошибку.
        context.camera.hold_in_background_for_diagnostics();
        let previous_session = running.handle().status().stream.session;
        if let Err(failure) = send_commands(running, &resolved) {
            return ScenarioStep::Finished(failed(failure.describe()));
        }
        context.emit_stage(
            "started",
            stage_fields([
                ("streaming_timeout_ms", config.streaming_timeout_ms.into()),
                ("eviction_timeout_ms", config.eviction_timeout_ms.into()),
                ("recovery_timeout_ms", config.recovery_timeout_ms.into()),
            ]),
        );
        self.run = Some(Run::new(config, context.now, previous_session));
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Ok(running) = context.camera.running() else {
            return ScenarioStep::Continue;
        };
        let Some(run) = &mut self.run else {
            return ScenarioStep::Continue;
        };
        match run.tick(running, context.now) {
            Progress::Continue => ScenarioStep::Continue,
            Progress::StreamingReached => {
                let camera = run.observation.camera.clone().unwrap_or_default();
                context.emit_stage("streaming", stage_fields([("camera", camera.into())]));
                ScenarioStep::Continue
            }
            Progress::Finished => match self.run.take() {
                Some(finished) => ScenarioStep::Finished(finished.into_outcome()),
                None => ScenarioStep::Continue,
            },
        }
    }
}
