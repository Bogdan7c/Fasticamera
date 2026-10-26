//! Сценарий `camera-stream`: подтвердить через NDK, что камеры реально дают поток с нужной частотой.
//!
//! По шагам из конфига (`camera.scenarios.stream_steps`) сценарий открывает камеру, просит режим,
//! запускает поток, ждёт `Streaming`, выдерживает замер и снимает статистику по меткам сенсора и
//! счётчики RAW-приёмника бэкенда. Шаги идут строго по очереди; **провал шага не останавливает
//! остальные** — отчёт целиком главная ценность прогона. Вердикт шага — [`evaluate_step`](super::stream_report::evaluate_step).
//!
//! Между шагами камера закрывается и сценарий ждёт `Closed`: иначе команда «открыть следующую
//! камеру» застала бы движок со старым режимом потока и породила бы ложный отказ конфигурации.
//! Все ожидания идут по тикам маршрутизатора с дедлайнами из конфига; поток маршрутизатора не блокируется.

use std::time::{Duration, Instant};

use camera_android::DiagnosticsSnapshot;
use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::CameraTarget;
use camera_core::{CameraEvent, CameraHandle, CameraState, CameraStatus, CommandId};

use super::detail::emit_details;
use super::engine_watch::{RunFailure, failure_of_event, fatal_failure, rejected_failure, unavailable_reasons};
use super::progress::{Deadline, EventLog};
use super::stream_report::{ConfiguredEntry, RawSinkDelta, StepObservation, StepReport, StreamReport};
use crate::camera_scenario_config::{ResolvedStep, SCENARIO_CAPTURE_INTENT, StreamStepConfig};
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, judged, stage_fields,
};

/// Сколько байт JSON отдаём сводке в строке результата: запись logcat — 4000 байт, часть уходит на префикс,
/// имя сценария и поле `error` с нарушениями.
const RESULT_SUMMARY_BUDGET_BYTES: usize = 3_000;

/// Шаг плана: конфиг (для отчёта и вердикта) и то же в виде команд движку.
struct PlannedStep {
    config: StreamStepConfig,
    resolved: ResolvedStep,
}

/// Сроки прогона, снятые с конфига на старте (конфиг фиксирован на процесс).
#[derive(Clone, Copy)]
struct Timeouts {
    start_ms: u64,
    close_ms: u64,
    measure_ms: u64,
}

/// Шаг, который выполняется сейчас.
struct StepRun {
    index: usize,
    events: EventLog,
    diagnostics_before: DiagnosticsSnapshot,
    unavailable: Vec<CameraUnavailableReason>,
}

/// Момент, когда поток пошёл.
struct StreamingReached {
    configured: ConfiguredEntry,
    after_ms: u64,
    measure_until: Instant,
    /// Когда поток пошёл: от этого момента считается частота по RAW-кадрам.
    at: Instant,
    diagnostics_at_streaming: Box<DiagnosticsSnapshot>,
}

/// Ожидание закрытия камеры перед шагом `next_index`. Недостаточно увидеть `Closed` в снимке: он мог
/// остаться от прошлого шага (камера и не открывалась), а команды прошлого шага ещё в очереди движка.
/// Поэтому ждём итог именно нашей команды `close`: раньше неё публикуются итоги всех прошлых команд.
struct ClosingWait {
    next_index: usize,
    deadline: Deadline,
    close_command: CommandId,
    close_acknowledged: bool,
}

enum Phase {
    /// Ждём, пока камера закроется перед следующим шагом.
    Closing(ClosingWait),
    /// Команды шага отправлены, ждём `Streaming`.
    Starting { step: StepRun, deadline: Deadline },
    /// Поток идёт, идёт замер.
    Measuring { step: StepRun, reached: StreamingReached },
    /// Все шаги выполнены.
    Done,
}

/// Состояние прогона: план, фаза и готовые отчёты шагов.
struct Run {
    plan: Vec<PlannedStep>,
    timeouts: Timeouts,
    phase: Phase,
    completed: Vec<StepReport>,
}

/// Стадия, которую надо показать скрипту после тика.
struct PendingStage {
    stage: &'static str,
    fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Default)]
pub struct CameraStreamScenario {
    run: Option<Run>,
}

impl CameraStreamScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Отправляет команды шага: открыть, режим, запуск потока. Первый отказ очереди — провал шага.
fn send_step_commands(handle: &CameraHandle, resolved: &ResolvedStep) -> Result<(), RunFailure> {
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

/// Забирает события и сообщает, пришёл ли итог команды `command`. Остальные события при ожидании
/// закрытия не нужны: они относятся к прошлому шагу.
fn close_was_handled(running: &RunningCamera, command: CommandId) -> bool {
    running
        .handle()
        .drain_events(usize::MAX)
        .iter()
        .any(|event| matches!(event, CameraEvent::CommandHandled { command: handled, .. } if *handled == command))
}

fn is_closed(status: &CameraStatus) -> bool {
    matches!(status.state, CameraState::Closed { .. })
}

/// Наш ли это `Streaming`: запрошенная камера и режим. Старого `Streaming` здесь быть не может: шаг начинается
/// только после подтверждённого закрытия камеры.
fn streaming_of_step(status: &CameraStatus, planned: &PlannedStep) -> Option<ConfiguredEntry> {
    let CameraState::Streaming { stream } = &status.state else {
        return None;
    };
    let wanted = &planned.resolved.mode;
    let actual = &stream.stream;
    let wanted_camera = matches!(&planned.resolved.target, CameraTarget::Camera(id) if *id == actual.camera);
    let same_stream = wanted_camera
        && actual.raw_format == wanted.raw_format
        && actual.readout == wanted.readout
        && actual.frame_rate == wanted.frame_rate;
    same_stream.then(|| ConfiguredEntry::of(stream))
}

impl Run {
    fn new(plan: Vec<PlannedStep>, timeouts: Timeouts, first_close: Deadline, close_command: CommandId) -> Self {
        Self {
            plan,
            timeouts,
            phase: Phase::Closing(ClosingWait {
                next_index: 0,
                deadline: first_close,
                close_command,
                close_acknowledged: false,
            }),
            completed: Vec::new(),
        }
    }

    /// Один тик: продвигает фазу, возвращает стадии для скрипта.
    fn tick(&mut self, running: &RunningCamera, now: Instant) -> Vec<PendingStage> {
        let mut stages = Vec::new();
        let status = running.handle().status();
        let phase = std::mem::replace(&mut self.phase, Phase::Done);
        self.phase = match phase {
            Phase::Closing(wait) => self.tick_closing(running, &status, now, wait, &mut stages),
            Phase::Starting { step, deadline } => {
                self.tick_starting(running, &status, now, step, deadline, &mut stages)
            }
            Phase::Measuring { step, reached } => {
                self.tick_measuring(running, &status, now, step, reached, &mut stages)
            }
            Phase::Done => Phase::Done,
        };
        stages
    }

    fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    fn tick_closing(
        &mut self,
        running: &RunningCamera,
        status: &CameraStatus,
        now: Instant,
        mut wait: ClosingWait,
        stages: &mut Vec<PendingStage>,
    ) -> Phase {
        wait.close_acknowledged = wait.close_acknowledged || close_was_handled(running, wait.close_command);
        if wait.close_acknowledged && is_closed(status) {
            return self.begin_step(running, now, wait.next_index, stages);
        }
        if wait.deadline.is_expired(now) {
            let failure = RunFailure::PreviousCameraDidNotClose {
                last_state: status.state.name(),
            };
            self.complete_without_stream(running, wait.next_index, failure);
            return self.after_step(running, wait.next_index, now);
        }
        Phase::Closing(wait)
    }

    /// Камера закрыта: отправляем команды шага `index`.
    fn begin_step(
        &mut self,
        running: &RunningCamera,
        now: Instant,
        index: usize,
        stages: &mut Vec<PendingStage>,
    ) -> Phase {
        // Хвост событий закрытия не относится к шагу.
        running.handle().drain_events(usize::MAX);
        let step = StepRun {
            index,
            events: EventLog::started_at(now),
            diagnostics_before: running.diagnostics_snapshot(),
            unavailable: Vec::new(),
        };
        let planned = &self.plan[index];
        if let Err(failure) = send_step_commands(running.handle(), &planned.resolved) {
            self.complete_without_stream(running, index, failure);
            return self.after_step(running, index, now);
        }
        stages.push(PendingStage {
            stage: "step_started",
            fields: stage_fields([("index", index.into()), ("target", planned.config.label().into())]),
        });
        Phase::Starting {
            step,
            deadline: Deadline::after_millis(now, self.timeouts.start_ms),
        }
    }

    fn tick_starting(
        &mut self,
        running: &RunningCamera,
        status: &CameraStatus,
        now: Instant,
        mut step: StepRun,
        deadline: Deadline,
        stages: &mut Vec<PendingStage>,
    ) -> Phase {
        let events = running.handle().drain_events(usize::MAX);
        step.events.absorb(now, &events);
        step.unavailable.extend(unavailable_reasons(&events));
        let failure = events
            .iter()
            .find_map(failure_of_event)
            .or_else(|| fatal_failure(status));
        let index = step.index;
        if let Some(failure) = failure {
            self.complete_step(running, status, now, step, None, Some(failure));
            return self.after_step(running, index, now);
        }
        if let Some(configured) = streaming_of_step(status, &self.plan[step.index]) {
            let after_ms = step.events.elapsed_ms(now);
            stages.push(PendingStage {
                stage: "step_streaming",
                fields: stage_fields([("index", step.index.into()), ("after_ms", after_ms.into())]),
            });
            let measure_until = now + Duration::from_millis(self.timeouts.measure_ms);
            return Phase::Measuring {
                step,
                reached: StreamingReached {
                    configured,
                    after_ms,
                    measure_until,
                    at: now,
                    diagnostics_at_streaming: Box::new(running.diagnostics_snapshot()),
                },
            };
        }
        if deadline.is_expired(now) {
            let failure = RunFailure::NeverStreaming {
                last_state: status.state.name(),
            };
            self.complete_step(running, status, now, step, None, Some(failure));
            return self.after_step(running, index, now);
        }
        Phase::Starting { step, deadline }
    }

    fn tick_measuring(
        &mut self,
        running: &RunningCamera,
        status: &CameraStatus,
        now: Instant,
        mut step: StepRun,
        reached: StreamingReached,
        stages: &mut Vec<PendingStage>,
    ) -> Phase {
        let events = running.handle().drain_events(usize::MAX);
        step.events.absorb(now, &events);
        step.unavailable.extend(unavailable_reasons(&events));
        let interrupted = !matches!(status.state, CameraState::Streaming { .. });
        if now < reached.measure_until && !interrupted {
            return Phase::Measuring { step, reached };
        }
        let index = step.index;
        self.complete_step(running, status, now, step, Some(reached), None);
        let measured = self
            .completed
            .last()
            .and_then(|report| report.observation.stats.as_ref());
        stages.push(PendingStage {
            stage: "step_measured",
            fields: stage_fields([
                ("index", index.into()),
                (
                    "measured_fps",
                    measured
                        .and_then(|stats| stats.measured_frames_per_second)
                        .map_or(serde_json::Value::Null, serde_json::Value::from),
                ),
            ]),
        });
        self.after_step(running, index, now)
    }

    /// Шаг, у которого нет шанса дойти до потока (не закрылась прошлая камера, очередь отказала).
    fn complete_without_stream(&mut self, running: &RunningCamera, index: usize, failure: RunFailure) {
        let observation = StepObservation {
            failure: Some(failure),
            configured: None,
            time_to_streaming_ms: None,
            stats: None,
            state_at_end: None,
            raw_sink: None,
            window_ms: None,
            raw_fps: None,
            callback_panics: running.diagnostics_snapshot().callback_panics,
            unmatched_frame_numbers: 0,
            unavailable: Vec::new(),
            events: Vec::new(),
            events_dropped: 0,
        };
        self.push_report(running, index, observation);
    }

    /// Завершает шаг с наблюдениями на этот момент (`reached` есть, если поток доходил до `Streaming`).
    fn complete_step(
        &mut self,
        running: &RunningCamera,
        status: &CameraStatus,
        now: Instant,
        step: StepRun,
        reached: Option<StreamingReached>,
        failure: Option<RunFailure>,
    ) {
        let after = running.diagnostics_snapshot();
        let (events, events_dropped) = StepObservation::capped_events(step.events.notes());
        // Вторая независимая оценка частоты: RAW-кадры с момента `Streaming` за фактическую длительность замера.
        let raw_rate = reached.as_ref().map(|reached| {
            let window = now.saturating_duration_since(reached.at);
            let frames = RawSinkDelta::between(&reached.diagnostics_at_streaming, &after).frames_acquired;
            (
                u64::try_from(window.as_millis()).unwrap_or(u64::MAX),
                RawSinkDelta::frames_per_second(frames, window),
            )
        });
        let observation = StepObservation {
            failure,
            time_to_streaming_ms: reached.as_ref().map(|reached| reached.after_ms),
            configured: reached.map(|reached| reached.configured),
            stats: Some(status.stream.clone()),
            state_at_end: Some(status.state.name()),
            raw_sink: Some(RawSinkDelta::between(&step.diagnostics_before, &after)),
            window_ms: raw_rate.map(|(window_ms, _)| window_ms),
            raw_fps: raw_rate.and_then(|(_, fps)| fps),
            callback_panics: after.callback_panics,
            unmatched_frame_numbers: after
                .unmatched_frame_numbers
                .saturating_sub(step.diagnostics_before.unmatched_frame_numbers),
            unavailable: step.unavailable,
            events,
            events_dropped,
        };
        self.push_report(running, step.index, observation);
    }

    fn push_report(&mut self, running: &RunningCamera, index: usize, observation: StepObservation) {
        let target = self.plan[index].config.clone();
        self.completed.push(StepReport::judge(
            index,
            target,
            observation,
            running.raw_reader_max_images(),
        ));
    }

    /// После шага `finished`: закрыть камеру и ждать закрытия перед следующим шагом, либо закончить.
    /// Команду `close` очередь может отвергнуть — тогда следующий шаг записывается проваленным.
    fn after_step(&mut self, running: &RunningCamera, finished: usize, now: Instant) -> Phase {
        let mut next = finished + 1;
        while next < self.plan.len() {
            match running.handle().close() {
                Ok(close_command) => {
                    return Phase::Closing(ClosingWait {
                        next_index: next,
                        deadline: Deadline::after_millis(now, self.timeouts.close_ms),
                        close_command,
                        close_acknowledged: false,
                    });
                }
                Err(rejected) => {
                    self.complete_without_stream(running, next, rejected_failure("close", rejected));
                    next += 1;
                }
            }
        }
        Phase::Done
    }
}

impl ObservingScenario for CameraStreamScenario {
    fn name(&self) -> &'static str {
        "camera-stream"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let running = match context.camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = context.camera.scenarios();
        if config.stream_steps.is_empty() {
            return ScenarioStep::Finished(failed("в конфиге нет шагов camera-stream"));
        }
        let mut plan = Vec::with_capacity(config.stream_steps.len());
        for step_config in &config.stream_steps {
            match step_config.resolve() {
                Ok(resolved) => plan.push(PlannedStep {
                    config: step_config.clone(),
                    resolved,
                }),
                Err(error) => return ScenarioStep::Finished(failed(format!("шаг {}: {error}", step_config.label()))),
            }
        }
        let timeouts = Timeouts {
            start_ms: config.stream_start_timeout_ms,
            close_ms: config.stream_close_timeout_ms,
            measure_ms: config.measure_ms,
        };
        // Камеру открывает сценарий, значит и закрыть её в конце (любым путём) обязан маршрутизатор.
        context.camera.claim_camera_for_scenario();
        context.emit_stage(
            "started",
            stage_fields([("steps", plan.len().into()), ("measure_ms", timeouts.measure_ms.into())]),
        );
        let close_command = match running.handle().close() {
            Ok(command) => command,
            Err(rejected) => {
                return ScenarioStep::Finished(failed(format!("движок не принял команду close: {rejected}")));
            }
        };
        let first_close = Deadline::after_millis(context.now, timeouts.close_ms);
        self.run = Some(Run::new(plan, timeouts, first_close, close_command));
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
        for pending in run.tick(running, context.now) {
            context.emit_stage(pending.stage, pending.fields);
        }
        if !run.is_done() {
            return ScenarioStep::Continue;
        }
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let report = StreamReport {
            measure_ms: finished.timeouts.measure_ms,
            raw_in_flight_limit: running.raw_reader_max_images(),
            steps: finished.completed,
            backend: running.diagnostics_snapshot(),
        };
        // Полный отчёт шагов — строками стадий; в строку результата (4000 байт) идёт сводка.
        emit_details(context, "detail_steps", report.detail_items());
        let violations = report.violations();
        ScenarioStep::Finished(judged(report.summary_within(RESULT_SUMMARY_BUDGET_BYTES), violations))
    }
}
