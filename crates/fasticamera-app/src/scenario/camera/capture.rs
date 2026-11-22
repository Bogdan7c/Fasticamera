//! Сценарий `capture` (сессия 14): снимок на НАСТОЯЩЕМ экране камеры до файла в галерее.
//!
//! Сценарий ничего не заявляет и не пересоздаёт: ведёт себя как пользователь. Экран стоит на месте
//! ([`CameraScreenPolicy::KeepRunning`]), затвор и значок RAW сценарий нажимает действиями экрана (тем же входом, что
//! касания и клавиши), а судит по тому, что видно снаружи: журнал итогов службы записи (адреса файлов, время фаз),
//! счётчики экрана и видоискателя, учёт команд снимка в рендере.
//!
//! # Ход
//! 1. **Ворота:** сервис снимка настоящий ([`CaptureMode::Real`]; фейк или запасной фейк с причиной — провал сразу),
//!    поток идёт, экран видит живую камеру, рендер показал ЖИВОЙ кадр активной камеры. Затем выдержка
//!    `viewfinder_settle_ms` (AE в начале потока даёт почти чёрные кадры).
//! 2. **Шаги** ([`planned_steps`]): `single_heif`, `single_heif_dng`, `burst` — устройство в [`CaptureRunner`].
//! 3. **Итог:** сводка с адресами файлов в строке результата (по ним скрипт ПК проверит и удалит файлы) и
//!    подробности `detail_capture`. Решает [`evaluate_capture`].
//!
//! Правило тишины: пока идёт сценарий, экран трогать нельзя.

use std::time::Instant;

use camera_core::CameraState;
use camera_screen::ScreenAction;
use camera_screen::settings::RawCapture;
use camera_screen::tools::ToolId;
use render_loop::still::StillCommandLog;
use render_loop::viewfinder::ViewfinderDiagnostics;

use super::capture_observation::{CaptureObservation, LiveCounters, SubmitCounts};
use super::capture_plan::planned_steps;
use super::capture_report::{CaptureObservationReport, RenderStillDelta};
use super::capture_runner::{CaptureRunner, RunnerState};
use super::capture_verdict::evaluate_capture;
use super::detail::emit_details;
use super::engine_watch::fatal_failure;
use super::result_budget::finish_within_budget;
use super::zoom_views::screen_shows_live_picture_of;
use crate::camera_presentation::CameraPresentationObservation;
use crate::camera_screen_input::{InputDelivery, ScreenInputInjector};
use crate::camera_screen_service::CameraScreenPolicy;
use crate::capture_backend::CaptureMode;
use crate::capture_scenario_config::CaptureScenarioConfig;
use crate::gpu_services::RenderObservation;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::still_phase_log::PhaseCursor;

/// Чем занят прогон.
enum Phase {
    /// Ждём готовности (срок — `ready_timeout_ms`).
    Preparing { started_at: Instant },
    /// Всё готово, выдерживаем `viewfinder_settle_ms`.
    Settling { since: Instant },
    /// Идут шаги.
    Stepping(Box<Stepping>),
}

/// Состояние шагов.
struct Stepping {
    runner: CaptureRunner,
    /// Курсор журнала итогов: всё, что до него, к прогону не относится.
    cursor: PhaseCursor,
    render_before: Option<StillCommandLog>,
}

struct CaptureRun {
    config: CaptureScenarioConfig,
    injector: ScreenInputInjector,
    phase: Phase,
    /// Вложения, которые канал входа не принял: нажатие потеряно — прогон проваливается.
    input_refusals: u64,
    mode_at_start: CaptureMode,
}

/// Сценарий `capture`.
#[derive(Default)]
pub struct CaptureScenario {
    run: Option<CaptureRun>,
}

impl CaptureScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservingScenario for CaptureScenario {
    fn name(&self) -> &'static str {
        "capture"
    }

    /// Экран остаётся на месте и получает вход; снимки — настоящие (фейк на время сценария не подставляется).
    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        CameraScreenPolicy::KeepRunning
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        if let Err(unavailable) = context.camera.running() {
            return ScenarioStep::Finished(failed(unavailable.to_string()));
        }
        let Some(injector) = context.camera_screen.input_injector() else {
            return ScenarioStep::Finished(failed(format!(
                "экран камеры не установлен (статус {:?}): нажимать затвор нечем",
                context.camera_screen.status()
            )));
        };
        // Ворота «снимки настоящие»: фейк вместо настоящего — прогон, который ничего не доказывает.
        let mode = context.camera_screen.capture_diagnostics().mode;
        if mode != CaptureMode::Real {
            return ScenarioStep::Finished(failed(not_real_reason(&mode)));
        }
        let config = context.camera.scenarios().capture.clone();
        context.emit_stage(
            "started",
            stage_fields([
                ("steps", planned_steps().len().into()),
                ("ready_timeout_ms", config.ready_timeout_ms.into()),
                ("burst_presses", config.burst.presses.into()),
                ("burst_interval_ms", config.burst.interval_ms.into()),
            ]),
        );
        self.run = Some(CaptureRun {
            config,
            injector,
            phase: Phase::Preparing {
                started_at: context.now,
            },
            input_refusals: 0,
            mode_at_start: mode,
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
        if let Some(failure) = fatal_failure(&status) {
            return self.finish(context, Some(failure.describe()));
        }
        let presentation = match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::Live(presentation) => presentation,
            CameraPresentationObservation::RenderUnavailable { reason } => {
                return self.finish(context, Some(format!("рендер недоступен: {reason}")));
            }
            CameraPresentationObservation::NotShowingCamera => {
                return self.wait_or_time_out(context, "рендер не показывает камеру");
            }
        };
        let Some(viewfinder) = presentation.viewfinder.as_ref() else {
            return self.wait_or_time_out(context, "рендер без видоискателя");
        };
        let Some(report) = context.camera_screen.report() else {
            return self.wait_or_time_out(context, "экран камеры ещё не нарисовал кадр");
        };
        let frames = &viewfinder.continuity.recent_frames;
        let Some(render_now) = frames.last().map(|frame| frame.present_time) else {
            return self.wait_or_time_out(context, "рендер ещё не показал ни одного кадра");
        };
        match &mut run.phase {
            Phase::Preparing { started_at } => {
                let started = *started_at;
                let streaming = matches!(status.state, CameraState::Streaming { .. });
                let live_on_screen = status
                    .lens
                    .active
                    .as_ref()
                    .is_some_and(|active| screen_shows_live_picture_of(frames, &active.camera));
                if streaming && report.presence == camera_screen::CameraPresence::Live && live_on_screen {
                    run.phase = Phase::Settling { since: context.now };
                    context.emit_stage("capture_ready", stage_fields([]));
                    return ScenarioStep::Continue;
                }
                self.time_out_since(
                    context,
                    started,
                    "камера или экран ещё не готовы, живого кадра активной камеры нет",
                )
            }
            Phase::Settling { since } => {
                let settled = context.now.saturating_duration_since(*since).as_millis()
                    >= u128::from(run.config.viewfinder_settle_ms);
                if settled {
                    run.phase = Phase::Stepping(Box::new(Stepping {
                        runner: CaptureRunner::new(run.config.clone(), planned_steps(), context.now),
                        cursor: context.camera_screen.capture_phase_cursor_now(),
                        render_before: still_commands(context),
                    }));
                }
                ScenarioStep::Continue
            }
            Phase::Stepping(stepping) => {
                let batch = context.camera_screen.capture_phases_after(stepping.cursor);
                stepping.cursor = batch.cursor_after(stepping.cursor);
                stepping.runner.absorb_phases(batch.entries, batch.lost_entries);
                let journal = context.camera_screen.capture_journal();
                let observation = CaptureObservation {
                    now: context.now,
                    render_now,
                    raw_setting: report.capture_settings.raw,
                    actions_rejected: report.diagnostics.actions_rejected,
                    submit: SubmitCounts {
                        accepted: report.effects.capture.accepted,
                        rejected: report.effects.capture.rejected_busy + report.effects.capture.rejected_unavailable,
                    },
                    journal: &journal,
                    live: live_counters(viewfinder, &presentation, context),
                    mode_is_real: context.camera_screen.capture_diagnostics().mode == CaptureMode::Real,
                };
                let tick = stepping.runner.tick(&observation);
                for action in tick.injections {
                    if run.injector.inject(action) != InputDelivery::Queued {
                        run.input_refusals += 1;
                    }
                }
                if run.input_refusals > 0 {
                    return self.finish(
                        context,
                        Some("канал входа экрана не принял вложение сценария".to_owned()),
                    );
                }
                match tick.state {
                    RunnerState::Running => ScenarioStep::Continue,
                    RunnerState::AllDone | RunnerState::Failed(_) => self.finish(context, None),
                }
            }
        }
    }
}

impl CaptureScenario {
    /// Ждать дальше, пока не вышел срок готовности; вышел — провал с причиной. На шагах пропавшее наблюдение —
    /// потеря посреди прогона.
    fn wait_or_time_out(&mut self, context: &mut ScenarioContext<'_>, waiting_for: &str) -> ScenarioStep {
        let started = match self.run.as_ref().map(|run| &run.phase) {
            Some(Phase::Preparing { started_at }) => *started_at,
            Some(Phase::Settling { since }) => *since,
            Some(Phase::Stepping(_)) => {
                return self.finish(
                    context,
                    Some(format!("посреди шагов пропало наблюдение: {waiting_for}")),
                );
            }
            None => return ScenarioStep::Continue,
        };
        self.time_out_since(context, started, waiting_for)
    }

    fn time_out_since(
        &mut self,
        context: &mut ScenarioContext<'_>,
        started: Instant,
        waiting_for: &str,
    ) -> ScenarioStep {
        let Some(run) = self.run.as_ref() else {
            return ScenarioStep::Continue;
        };
        let waited = context.now.saturating_duration_since(started).as_millis();
        if waited >= u128::from(run.config.ready_timeout_ms) {
            return self.finish(context, Some(format!("не готово за {waited} мс: {waiting_for}")));
        }
        ScenarioStep::Continue
    }

    /// Завершение: собрать отчёт, вывести подробности и вердикт.
    fn finish(&mut self, context: &mut ScenarioContext<'_>, run_failure: Option<String>) -> ScenarioStep {
        let Some(run) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let render_after = still_commands(context);
        let restore_failure = restore_raw_if_left_on(&run, context);
        let report = observation_of(run, context, run_failure.or(restore_failure), render_after);
        emit_details(context, "detail_capture", report.detail_items());
        let config = context.camera.scenarios().capture.clone();
        let violations = evaluate_capture(&config, &report);
        ScenarioStep::Finished(finish_within_budget(context, report.summary(), violations))
    }
}

/// Шаг «HEIF + DNG» включает RAW; если прогон оборвался до серии, настройка осталась бы включённой у владельца.
/// Возвращаем её тем же значком. `Some(текст)` — вернуть не удалось (канал входа не принял действие).
fn restore_raw_if_left_on(run: &CaptureRun, context: &ScenarioContext<'_>) -> Option<String> {
    let raw_is_on = context
        .camera_screen
        .report()
        .is_some_and(|report| report.capture_settings.raw == RawCapture::On);
    if !raw_is_on {
        return None;
    }
    match run.injector.inject(ScreenAction::StripToolTapped(ToolId::Raw)) {
        InputDelivery::Queued => None,
        refused => Some(format!("RAW остался включённым: значок не нажат ({refused:?})")),
    }
}

/// Почему сценарий не стартует: режим сервиса снимка не настоящий.
fn not_real_reason(mode: &CaptureMode) -> String {
    match mode {
        CaptureMode::FakeFallback { reason } => {
            format!("сервис снимка не настоящий: запасной фейк, настоящий не поднялся ({reason})")
        }
        other => format!("сервис снимка не настоящий: {other:?}"),
    }
}

/// Учёт команд снимка в рендере; `None` — рендер недоступен.
fn still_commands(context: &ScenarioContext<'_>) -> Option<StillCommandLog> {
    match context.gpu.render.observe() {
        RenderObservation::Live { status, .. } => Some(status.still_commands),
        RenderObservation::Unavailable { .. } => None,
    }
}

/// Счётчики видоискателя для окна серии: рендер (непрерывность, пейсинг) и поток кадров камеры.
fn live_counters(
    viewfinder: &ViewfinderDiagnostics,
    presentation: &crate::camera_presentation::CameraPresentation,
    context: &ScenarioContext<'_>,
) -> LiveCounters {
    let raw_stream = context.camera.raw_stream().counters();
    let continuity = &viewfinder.continuity.counters;
    let shift = &presentation.present_target.vsync_shift;
    LiveCounters {
        pool_exhausted: raw_stream.dropped_pool_exhausted,
        jitter_overflow: raw_stream.dropped_jitter_overflow,
        underruns: raw_stream.viewfinder_underruns,
        // Корзина 0 — вовремя; всё, что позже задуманного vsync, — опоздавшие кадры (число, не провал: решение 91).
        late_frames: shift.late_by_periods.iter().skip(1).sum(),
        black_frames_while_picture_expected: continuity.black_frames_while_picture_expected,
        holds_started: continuity.holds_started,
        live_frames_presented: continuity.live_frames_presented,
    }
}

/// Приращение учёта рендера за прогон.
fn render_delta(before: Option<StillCommandLog>, after: Option<StillCommandLog>) -> Option<RenderStillDelta> {
    let (before, after) = (before?, after?);
    Some(RenderStillDelta {
        begins_accepted: after.begins_accepted.saturating_sub(before.begins_accepted),
        begins_unsupported: after.begins_unsupported.saturating_sub(before.begins_unsupported),
        targets_accepted: after.targets_accepted.saturating_sub(before.targets_accepted),
        targets_dropped: after.targets_dropped.saturating_sub(before.targets_dropped),
        lost_to_backend_fault: after.lost_to_backend_fault.saturating_sub(before.lost_to_backend_fault),
    })
}

/// Наблюдение на конец прогона.
fn observation_of(
    run: CaptureRun,
    context: &ScenarioContext<'_>,
    run_failure: Option<String>,
    render_after: Option<StillCommandLog>,
) -> CaptureObservationReport {
    let planned = planned_steps();
    let (records, render_before) = match run.phase {
        Phase::Stepping(stepping) => {
            let Stepping {
                runner, render_before, ..
            } = *stepping;
            (Some(runner.into_records()), render_before)
        }
        Phase::Preparing { .. } | Phase::Settling { .. } => (None, None),
    };
    let (steps, shots, burst, unfinished, runner_failure, phase_entries_lost) = match records {
        Some(records) => (
            records.steps,
            records.shots,
            records.burst,
            records.unfinished,
            records.failure,
            records.phase_entries_lost,
        ),
        None => (
            Vec::new(),
            Vec::new(),
            None,
            planned.iter().map(|step| step.name).collect(),
            None,
            0,
        ),
    };
    let input_failure = (run.input_refusals > 0).then(|| "канал входа экрана не принял вложение сценария".to_owned());
    CaptureObservationReport {
        planned_steps: planned.len(),
        steps,
        shots,
        burst,
        unfinished,
        run_failure: run_failure.or(runner_failure).or(input_failure),
        mode_at_start: run.mode_at_start,
        mode_at_end: context.camera_screen.capture_diagnostics().mode,
        phase_entries_lost,
        render_still: render_delta(render_before, render_after),
        render_still_detail: None,
        full_frame: run.config.full_frame,
    }
}
