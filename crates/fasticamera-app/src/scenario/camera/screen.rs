//! Сценарий `screen` (сессия 12): экран камеры iOS поверх живого видоискателя, на котором скрипт делает касания.
//!
//! Экран камеры постоянно стоит поверх видоискателя, а на время обычного сценария снимается; этот сценарий —
//! единственный, кто проверяет сам экран кнопками затвора: он объявляет [`CameraScreenPolicy::KeepRunningWithFakeCapture`]
//! (экран на месте, снимки на время сценария — фейк, файлы в галерею владельца не пишутся).
//!
//! # Ход
//! 1. Общий запуск и замер видоискателя ([`SmokeRun`], как у `glass`): заявить камеру, открыть, дождаться кадров.
//! 2. **Прогрев:** экран увидел живую камеру и нарисовал `warmup_ui_frames` кадров (касания раньше теряются или
//!    отвергаются как «камера не живая»). Окно замера перезапускается, выдаётся стадия `screen_ready`.
//! 3. **Шаги** ([`planned_steps`]): на каждом сценарий выдаёт стадию `screen_gesture` с жестом (драйвер выполняет его
//!    по `adb`) и ждёт следствий на экране и в камере ([`StepRunner`]). Сценарий кончается, когда прошли все шаги.
//! 4. Итог: `detail_screen` и результат с вердиктом [`evaluate_screen`].
//!
//! Видоискатель сценарий не регулирует. Экран не снимается и не пересоздаётся: после сценария он остаётся тем же,
//! поэтому план заканчивается нейтрально (блокировка снята, панели закрыты, EV сброшен).

use std::time::Instant;

use render_loop::ui_overlay::UiOverlayDiagnostics;

use super::detail::emit_details;
use super::engine_watch::RunFailure;
use super::glass_report::{OverlayWindow, UiGpuBudget, WarmupReport};
use super::result_budget::finish_within_budget;
use super::screen_probe::ScreenProbe;
use super::screen_report::{ScreenCounters, ScreenObservation, evaluate_screen};
use super::screen_runner::{RunnerTick, StepRecord, StepRunner, StepTiming};
use super::screen_steps::planned_steps;
use super::viewfinder_metrics::round2;
use super::viewfinder_smoke::{RunStep, SmokeRun};
use crate::camera_screen_service::{CameraScreenPolicy, DroppedInputCounters};
use crate::camera_screen_source::ScreenReport;
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::screen_scenario_config::ScreenScenarioConfig;
use crate::viewfinder_scenario_config::{ViewfinderScenarioConfig, ViewfinderSmokeConfig};

/// Имя стадии, на которой драйвер выполняет жест (контракт с `scripts/smoke_driver_screen.py`).
pub const GESTURE_STAGE: &str = "screen_gesture";

/// Чем занят сценарий.
enum Phase {
    /// Видоискатель ещё запускается.
    AwaitingViewfinder,
    /// Видоискатель живой; ждём, пока экран увидит камеру и нарисует прогревочные кадры.
    WarmingUp {
        started_at: Instant,
        /// Сколько кадров экран нарисовал к началу прогрева.
        frames_at_start: u64,
    },
    /// Идут шаги плана.
    Stepping(Box<Stepping>),
}

/// Состояние на время шагов: исполнитель и базовые линии, от которых считается окно.
struct Stepping {
    runner: StepRunner,
    report_at_ready: ScreenReport,
    dropped_at_ready: DroppedInputCounters,
    overlay_at_ready: UiOverlayDiagnostics,
    unknown_origin_at_ready: u64,
    warmup: WarmupReport,
}

/// Состояние прогона.
struct ScreenRun {
    smoke: SmokeRun,
    config: ScreenScenarioConfig,
    smoke_config: ViewfinderSmokeConfig,
    phase: Phase,
}

/// Сценарий `screen`.
#[derive(Default)]
pub struct ScreenScenario {
    run: Option<ScreenRun>,
}

impl ScreenScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Диагностика слоя UI из последнего снимка показа (`None` — рендер видоискателя ещё не показывал).
fn ui_overlay_of(smoke: &SmokeRun) -> Option<&UiOverlayDiagnostics> {
    smoke
        .last_presentation()
        .and_then(|presentation| presentation.viewfinder.as_ref())
        .map(|viewfinder| &viewfinder.ui_overlay)
}

/// Целых мс от `since` до `now` (часы не идут назад: меньше — 0).
fn elapsed_ms(since: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(since).as_millis()).unwrap_or(u64::MAX)
}

/// Чем кончился тик.
enum Advance {
    Continue,
    /// План прошёл целиком (`None`) или провалился с причиной.
    Done(Option<String>),
}

impl ScreenRun {
    /// Один тик после тика видоискателя. Ошибка — готовый текст провала прогона.
    fn advance(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> Result<Advance, String> {
        if !context.camera_screen.status().is_installed() {
            return Err(format!(
                "экран камеры не стоит на рендере: статус {:?}",
                context.camera_screen.status()
            ));
        }
        match &mut self.phase {
            Phase::AwaitingViewfinder => {
                if self.smoke.is_measuring() {
                    let frames_at_start = context
                        .camera_screen
                        .report()
                        .map_or(0, |report| report.diagnostics.frames_produced);
                    self.phase = Phase::WarmingUp {
                        started_at: context.now,
                        frames_at_start,
                    };
                }
                Ok(Advance::Continue)
            }
            Phase::WarmingUp { .. } => self.advance_warmup(context, running),
            Phase::Stepping(_) => Ok(self.advance_steps(context, running)),
        }
    }

    /// Тик прогрева: экран увидел живую камеру и нарисовал нужные кадры — окно замера начинается заново,
    /// выдаётся `screen_ready`, строится исполнитель шагов.
    fn advance_warmup(
        &mut self,
        context: &mut ScenarioContext<'_>,
        running: &RunningCamera,
    ) -> Result<Advance, String> {
        let Phase::WarmingUp {
            started_at,
            frames_at_start,
        } = self.phase
        else {
            return Ok(Advance::Continue);
        };
        let status = running.handle().status();
        let report = context.camera_screen.report();
        let warmed = report.as_ref().map_or(0, |report| {
            report.diagnostics.frames_produced.saturating_sub(frames_at_start)
        });
        let sees_live_camera = report
            .as_ref()
            .is_some_and(|report| ScreenProbe::from_parts(report, &status, 0).presence_live);
        let ready = sees_live_camera && warmed >= self.config.warmup_ui_frames && ui_overlay_of(&self.smoke).is_some();
        if !ready {
            let waited_ms = elapsed_ms(started_at, context.now);
            if waited_ms >= self.config.warmup_timeout_ms {
                return Err(self.warmup_failure_reason(report.as_ref(), warmed, waited_ms));
            }
            return Ok(Advance::Continue);
        }
        let (Some(report), Some(overlay)) = (report, ui_overlay_of(&self.smoke).cloned()) else {
            return Err("экран или слой UI пропали посреди прогрева".to_owned());
        };
        // Кадры прогрева в окно замера не входят: окно и базовые линии — с этого момента.
        self.smoke
            .restart_window(context)
            .map_err(|error| format!("прогрев экрана закончен, но окно замера не перезапущено: {error}"))?;
        let probe = ScreenProbe::from_parts(&report, &status, context.camera_screen.capture_requests_accepted());
        let timing = StepTiming {
            timeout: std::time::Duration::from_millis(self.config.step_timeout_ms),
            settle: std::time::Duration::from_millis(self.config.step_settle_ms),
        };
        let steps = planned_steps(&self.config.touch);
        let step_count = steps.len();
        let warmup = WarmupReport {
            frames: warmed,
            duration_ms: elapsed_ms(started_at, context.now),
        };
        self.phase = Phase::Stepping(Box::new(Stepping {
            runner: StepRunner::new(steps, timing, probe),
            report_at_ready: report,
            dropped_at_ready: context.camera_screen.dropped_input(),
            overlay_at_ready: overlay,
            unknown_origin_at_ready: status.three_a.counters.results_with_unknown_origin,
            warmup,
        }));
        context.emit_stage(
            "screen_ready",
            stage_fields([
                ("steps", step_count.into()),
                ("warmup_frames", warmup.frames.into()),
                ("warmup_ms", warmup.duration_ms.into()),
                ("step_timeout_ms", self.config.step_timeout_ms.into()),
            ]),
        );
        Ok(Advance::Continue)
    }

    /// Текст провала прогрева: что экран видел и чего не хватило.
    fn warmup_failure_reason(&self, report: Option<&ScreenReport>, warmed: u64, waited_ms: u64) -> String {
        let Some(report) = report else {
            return format!(
                "экран камеры не нарисовал ни одного кадра за {waited_ms} мс прогрева (источник UI не работает?)"
            );
        };
        let mut reason = format!(
            "экран не готов за {waited_ms} мс прогрева: нарисовано {warmed} кадров из {}, камера для экрана {:?}",
            self.config.warmup_ui_frames, report.presence
        );
        if let Some(error) = &report.diagnostics.last_failure {
            reason.push_str(&format!("; последний отказ кадра: {error}"));
        }
        reason
    }

    /// Тик шагов: исполнитель решает, выдавать ли стадию с жестом.
    fn advance_steps(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> Advance {
        let Phase::Stepping(stepping) = &mut self.phase else {
            return Advance::Continue;
        };
        let status = running.handle().status();
        let probe = match ScreenProbe::read(context.camera_screen, &status) {
            Ok(probe) => probe,
            Err(_) => return Advance::Done(Some("у экрана пропал отчёт посреди шагов".to_owned())),
        };
        match stepping.runner.tick(context.now, &probe) {
            RunnerTick::Waiting => Advance::Continue,
            RunnerTick::AllDone => Advance::Done(None),
            RunnerTick::Failed(reason) => Advance::Done(Some(reason)),
            RunnerTick::BeginStep(step) => {
                context.emit_stage(
                    GESTURE_STAGE,
                    stage_fields([
                        ("step", step.name.into()),
                        ("purpose", step.purpose.into()),
                        ("gesture", step.gesture.to_stage_value()),
                    ]),
                );
                Advance::Continue
            }
        }
    }

    /// Собирает наблюдение на конец прогона.
    fn into_observation(
        self,
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        failure: Option<RunFailure>,
        plan_failure: Option<String>,
    ) -> ScreenObservation {
        let refresh_hz = f64::from(context.config.preferred_frame_rate_hz);
        let budget = UiGpuBudget {
            refresh_hz,
            fraction: self.config.ui_gpu_budget_fraction,
            budget_ms: round2(self.config.ui_gpu_budget_ms(refresh_hz)),
        };
        let overlay_after = ui_overlay_of(&self.smoke).cloned();
        let end_report = context.camera_screen.report();
        let unknown_origin_now = running.handle().status().three_a.counters.results_with_unknown_origin;
        let (steps, unfinished, screen, overlay, warmup) = match self.phase {
            Phase::Stepping(mut stepping) => {
                let records: Vec<StepRecord> = stepping.runner.records().to_vec();
                let unfinished = if plan_failure.is_some() {
                    Vec::new()
                } else {
                    stepping.runner.unfinished_names()
                };
                let screen = end_report.as_ref().map_or_else(ScreenCounters::default, |end| {
                    ScreenCounters::between(
                        &stepping.report_at_ready,
                        end,
                        (stepping.dropped_at_ready, context.camera_screen.dropped_input()),
                        unknown_origin_now.saturating_sub(stepping.unknown_origin_at_ready),
                    )
                });
                let overlay = overlay_after.as_ref().map_or_else(OverlayWindow::default, |after| {
                    OverlayWindow::between(&stepping.overlay_at_ready, after)
                });
                (records, unfinished, screen, overlay, stepping.warmup)
            }
            // Шаги не начинались (отказ камеры или прогрева раньше): окна экрана не было.
            Phase::AwaitingViewfinder | Phase::WarmingUp { .. } => (
                Vec::new(),
                Vec::new(),
                ScreenCounters::default(),
                OverlayWindow::default(),
                WarmupReport::default(),
            ),
        };
        ScreenObservation {
            viewfinder: self.smoke.into_observation(context, running, failure),
            steps,
            unfinished,
            plan_failure,
            screen,
            overlay,
            warmup,
            budget,
        }
    }
}

impl ObservingScenario for ScreenScenario {
    fn name(&self) -> &'static str {
        "screen"
    }

    /// Единственный сценарий, который проверяет сам экран: он остаётся на месте и получает вход.
    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        CameraScreenPolicy::KeepRunningWithFakeCapture
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        if !context.camera_screen.status().is_installed() {
            return ScenarioStep::Finished(failed(format!(
                "экран камеры не установлен (статус {:?}): проверять нечего",
                context.camera_screen.status()
            )));
        }
        let screen_config = camera.scenarios().screen.clone();
        // Потолок сценария — окно замера видоискателя; конец наступает раньше, когда пройдены все шаги.
        let viewfinder_config = ViewfinderScenarioConfig {
            smoke: ViewfinderSmokeConfig {
                measure_ms: screen_config.max_duration_ms,
                ..camera.scenarios().viewfinder.smoke.clone()
            },
            ..camera.scenarios().viewfinder.clone()
        };
        let smoke_config = viewfinder_config.smoke.clone();
        let smoke = match SmokeRun::begin(context, running, viewfinder_config) {
            Ok(smoke) => smoke,
            Err(reason) => return ScenarioStep::Finished(failed(reason)),
        };
        let refresh_hz = f64::from(context.config.preferred_frame_rate_hz);
        context.emit_stage(
            "started",
            stage_fields([
                ("max_duration_ms", screen_config.max_duration_ms.into()),
                ("steps", planned_steps(&screen_config.touch).len().into()),
                ("ui_gpu_budget_ms", screen_config.ui_gpu_budget_ms(refresh_hz).into()),
            ]),
        );
        self.run = Some(ScreenRun {
            smoke,
            config: screen_config,
            smoke_config,
            phase: Phase::AwaitingViewfinder,
        });
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let (Ok(running), Some(run)) = (camera.running(), self.run.as_mut()) else {
            return ScenarioStep::Continue;
        };
        let smoke_step = run.smoke.tick(context, running);
        let (failure, plan_failure) = match smoke_step {
            // Окно кончилось (потолок) или поток сломался: итог по тому, что успели.
            RunStep::Finish(failure) => (failure, None),
            RunStep::Continue => match run.advance(context, running) {
                Ok(Advance::Continue) => return ScenarioStep::Continue,
                Ok(Advance::Done(plan_failure)) => (None, plan_failure),
                Err(reason) => return self.finish_failed(reason),
            },
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let smoke_config = finished.smoke_config.clone();
        let observation = finished.into_observation(context, running, failure, plan_failure);
        emit_details(context, "detail_screen", observation.detail_items());
        let violations = evaluate_screen(&smoke_config, &observation);
        let summary = observation.summary();
        ScenarioStep::Finished(finish_within_budget(context, summary, violations))
    }
}

impl ScreenScenario {
    /// Прогон оборван с причиной до наблюдения (экран пропал, прогрев не удался).
    fn finish_failed(&mut self, reason: String) -> ScenarioStep {
        self.run = None;
        ScenarioStep::Finished(failed(reason))
    }
}
