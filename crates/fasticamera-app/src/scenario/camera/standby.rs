//! Сценарий `standby` (13b): «тёплая» вторая камера на настоящем телефоне — тёплая поднимается, её кадры не
//! показываются, обмен ролей без закрытия камер, одна открытая камера после отпуска.
//!
//! Сценарий не заявляет камеру и не трогает экран: он кладёт намерения объектива прямо в ящик движка
//! ([`camera_contract::zoom::LensIntentChannel`], тем же путём, что и экран) и судит по тому, что видно снаружи: статус и
//! события движка, счётчики потока кадров, диагностика бэкенда. Устройство шагов — [`StandbyRunner`], вердикт —
//! [`evaluate_standby`], отчёт — [`StandbyObservationReport`].
//!
//! Экран остаётся на месте ([`CameraScreenPolicy::KeepRunning`], как у `zoom`): продуктовый видоискатель идёт, как у
//! пользователя. Правило тишины: пока идёт сценарий, экран трогать нельзя.

use camera_core::{CameraEvent, CameraState};
use raw_frame_stream::RawStreamCounters;

use super::detail::emit_details;
use super::engine_watch::{failure_of_event, fatal_failure};
use super::result_budget::finish_within_budget;
use super::standby_report::{EngineTotals, StandbyObservationReport, StreamTotals};
use super::standby_runner::{RunnerState, StandbyObservation, StandbyRunner};
use super::standby_verdict::evaluate_standby;
use super::standby_views::StandbyEngineView;
use crate::camera_screen_service::CameraScreenPolicy;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};

/// Счётчики на начало прогона: суммы считаются от них.
struct Baselines {
    stream: RawStreamCounters,
    engine: StandbyEngineView,
}

/// Состояние прогона.
struct StandbyRun {
    runner: StandbyRunner,
    baselines: Baselines,
    /// Последнее состояние движка: по нему строится отчёт, если движок уже недоступен.
    last_engine: StandbyEngineView,
    /// Причина, по которой прогон кончился вне исполнителя (движок в ошибке, камера пропала).
    external_failure: Option<String>,
}

/// Сценарий `standby`.
#[derive(Default)]
pub struct StandbyScenario {
    run: Option<StandbyRun>,
}

impl StandbyScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservingScenario for StandbyScenario {
    fn name(&self) -> &'static str {
        "standby"
    }

    /// Как `zoom`: экран остаётся на месте, продуктовый видоискатель идёт.
    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        CameraScreenPolicy::KeepRunning
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if let Err(error) = context.config.standby.validate() {
            return ScenarioStep::Finished(failed(error.to_string()));
        }
        let running = match context.camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        // Хвост событий прошлых сценариев и автозапуска к этому прогону не относится: дальше события забираем мы.
        let _stale = running.handle().drain_events(usize::MAX);
        let status = running.handle().status();
        let engine = StandbyEngineView::of(&status);
        let config = context.config.standby;
        context.emit_stage(
            "started",
            stage_fields([
                ("home_timeout_ms", config.home_timeout_ms.into()),
                ("both_window_ms", config.both_window_ms.into()),
                ("min_back_fps", config.min_back_fps.into()),
            ]),
        );
        self.run = Some(StandbyRun {
            runner: StandbyRunner::new(config, context.now),
            baselines: Baselines {
                stream: context.camera.raw_stream().counters(),
                engine: engine.clone(),
            },
            last_engine: engine,
            external_failure: None,
        });
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(run) = self.run.as_mut() else {
            return ScenarioStep::Continue;
        };
        let Ok(running) = context.camera.running() else {
            return ScenarioStep::Continue;
        };
        let status = running.handle().status();
        let events = running.handle().drain_events(usize::MAX);
        if let Some(reason) = engine_failure(&status, &events) {
            run.external_failure = Some(reason);
            return self.finish(context);
        }
        let engine = StandbyEngineView::of(&status);
        let stream = context.camera.raw_stream().counters();
        let tick = run.runner.tick(&StandbyObservation {
            now: context.now,
            engine: &engine,
            events: &events,
            stream: &stream,
        });
        run.last_engine = engine;
        // Намерения — в ящик движка (последнее значение побеждает; тик публикует их по порядку).
        for intent in tick.intents {
            let (_generation, _delivery) = context.camera.lens_intents().publish(intent);
        }
        match tick.state {
            RunnerState::Running => ScenarioStep::Continue,
            RunnerState::AllDone | RunnerState::Failed(_) => self.finish(context),
        }
    }
}

impl StandbyScenario {
    /// Завершение: собрать отчёт, вывести подробности и вердикт.
    fn finish(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let Some(run) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let report = report_of(run, context);
        emit_details(context, "detail_standby", report.detail_items());
        let violations = evaluate_standby(&context.config.standby, &report);
        ScenarioStep::Finished(finish_within_budget(context, report.summary(), violations))
    }
}

/// Отказ движка, который делает прогон бессмысленным: окончательная ошибка или отказ команды.
fn engine_failure(status: &camera_core::CameraStatus, events: &[CameraEvent]) -> Option<String> {
    events
        .iter()
        .find_map(failure_of_event)
        .or_else(|| fatal_failure(status))
        .map(|failure| failure.describe())
}

/// Отчёт на конец прогона: приращения от базовых линий.
fn report_of(run: StandbyRun, context: &ScenarioContext<'_>) -> StandbyObservationReport {
    let running = context.camera.running().ok();
    let engine = running
        .map(|running| StandbyEngineView::of(&running.handle().status()))
        .unwrap_or(run.last_engine);
    let stream = context.camera.raw_stream().counters();
    let backend = running
        .map(|running| running.diagnostics_snapshot().standby)
        .unwrap_or_default();
    let final_state_failure = running.and_then(|running| match running.handle().status().state {
        CameraState::Error { .. } => Some("движок камеры в ошибке в конце прогона".to_owned()),
        _ => None,
    });
    let run_failure = run
        .external_failure
        .or(match run.runner.outcome_failure() {
            Some(reason) => Some(reason),
            // Исполнитель не дошёл до конца, а отказа нет: сценарий завершили снаружи.
            None if !run.runner.is_done() => Some(format!("прогон оборван на шаге {}", run.runner.stage_name())),
            None => None,
        })
        .or(final_state_failure);
    StandbyObservationReport {
        run_failure,
        stage: run.runner.stage_name(),
        record: run.runner.record().clone(),
        stream: stream_totals(&run.baselines.stream, &stream),
        engine: engine_totals(&run.baselines.engine, &engine),
        backend,
        final_role: engine.active.as_ref().map(|active| active.role.name()),
    }
}

fn stream_totals(before: &RawStreamCounters, after: &RawStreamCounters) -> StreamTotals {
    let delta = super::raw_stream_deltas::stream_delta(before, after);
    StreamTotals {
        standby_streams_begun: delta.standby_streams_begun,
        standby_begins_refused: delta.standby_begins_refused,
        standby_frames_returned: delta.standby_frames_returned,
        standby_promotions: delta.standby_promotions,
        frames_released_by_promotion: delta.frames_released_by_promotion,
        standby_results_evicted: delta.standby_results_evicted,
        standby_pool_exhausted: delta.standby_pool_exhausted,
        leaked_leases: delta.leaked_leases,
        frames_leaked: delta.frames_leaked,
    }
}

fn engine_totals(before: &StandbyEngineView, after: &StandbyEngineView) -> EngineTotals {
    let grew = |before: u64, after: u64| after.saturating_sub(before);
    EngineTotals {
        standby_opened: grew(before.standby_counters.opened, after.standby_counters.opened),
        promotions: grew(before.standby_counters.promotions, after.standby_counters.promotions),
        promotion_fallbacks: grew(
            before.standby_counters.promotion_fallbacks,
            after.standby_counters.promotion_fallbacks,
        ),
        standby_failures: grew(before.standby_counters.failures, after.standby_counters.failures),
        results_dropped_after_swap: grew(
            before.standby_counters.results_dropped_after_swap,
            after.standby_counters.results_dropped_after_swap,
        ),
        switches_completed: grew(
            before.lens_counters.switches_completed,
            after.lens_counters.switches_completed,
        ),
        switches_failed: grew(
            before.lens_counters.switches_failed,
            after.lens_counters.switches_failed,
        ),
        intents_ignored: grew(
            before.lens_counters.intents_ignored,
            after.lens_counters.intents_ignored,
        ),
        stale_backend_events: grew(before.stale_backend_events, after.stale_backend_events),
        dropped_capture_events: grew(before.dropped_capture_events, after.dropped_capture_events),
    }
}

#[cfg(test)]
#[path = "standby_tests.rs"]
mod tests;
