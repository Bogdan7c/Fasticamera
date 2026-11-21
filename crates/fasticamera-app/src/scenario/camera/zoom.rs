//! Сценарий `zoom` (сессия 13): переключение объективов и зум на НАСТОЯЩЕМ экране камеры поверх живого видоискателя.
//!
//! Сценарий не заявляет камеру и не пересоздаёт поток: он ведёт себя как пользователь. Экран стоит на месте
//! ([`CameraScreenPolicy::KeepRunning`]), сценарий вкладывает в него действия (тап по кнопке ленты, переворот) и
//! настоящие образцы касаний (протяжка по ленте — единственный путь, на котором колесо играет вибрации), а судит по
//! тому, что видно снаружи: события и статус движка, кольцо показанных кадров видоискателя, счётчики исполнителя
//! эффектов. Само устройство шагов — [`ZoomRunner`], вердикт — [`evaluate_zoom`], отчёт — [`ZoomObservationReport`].
//!
//! # Ход
//! 1. **Готовность:** поток идёт, каталог зума опубликован, экран видит живую камеру и нарисовал кадр, рендер отдаёт
//!    непрерывность (кольцо кадров). Срок — `ready_timeout_ms`. Затем исполнитель сам приводит экран к дому (задняя
//!    основная камера, 1×, колесо закрыто) и после провала возвращает его туда же (`zoom_home`).
//! 2. **Шаги** ([`planned_steps`]): `.6, 1, 2, 1, .6` → переворот на фронталку и назад → `1` → протяжка по ленте →
//!    завершающий `1`. На каждом шаге записывается время «действие → смена объектива» и «действие → первый живой кадр
//!    новой камеры», удержанные и чёрные кадры, частота, зеркало.
//! 3. **Итог:** `detail_zoom` и результат с вердиктом. Экран и камера остаются на 1× основной камеры.
//!
//! Единственный, кто забирает события движка, — этот сценарий (хвост прежних событий сбрасывается на старте).
//! Правило тишины: пока идёт сценарий, экран трогать нельзя — касания пальца идут тем же каналом входа.

use std::time::Instant;

use camera_core::{CameraEvent, CameraState};
use render_loop::viewfinder::ContinuityCounters;
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchSample};

use super::detail::emit_details;
use super::engine_watch::{failure_of_event, fatal_failure};
use super::result_budget::finish_within_budget;
use super::zoom_plan::planned_steps;
use super::zoom_report::{FinalState, ZoomObservationReport, ZoomTotals};
use super::zoom_runner::{EngineView, Injection, RunnerState, ScreenView, TouchInjection, ZoomObservation, ZoomRunner};
use super::zoom_verdict::evaluate_zoom;
use super::zoom_views::screen_shows_live_picture_of;
use crate::camera_presentation::CameraPresentationObservation;
use crate::camera_screen_input::{InputDelivery, ScreenInputInjector};
use crate::camera_screen_service::CameraScreenPolicy;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::zoom_scenario_config::ZoomScenarioConfig;

/// Базовые линии на момент готовности: суммы прогона считаются от них.
struct Baselines {
    continuity: ContinuityCounters,
    switches_completed: u64,
    switches_failed: u64,
    switches_retargeted: u64,
    intents_ignored: u64,
    lens_published: u64,
}

/// Чем занят прогон.
enum Phase {
    /// Ждём готовности (срок — `ready_timeout_ms`).
    Preparing { started_at: Instant },
    /// Идут шаги.
    Stepping {
        runner: Box<ZoomRunner>,
        baselines: Box<Baselines>,
    },
}

/// Состояние прогона.
struct ZoomRun {
    config: ZoomScenarioConfig,
    injector: ScreenInputInjector,
    phase: Phase,
    /// Вложения, которые канал входа не принял (переполнение, получателя нет): образец потерян — прогон проваливается.
    input_refusals: u64,
    planned_steps: usize,
}

/// Сценарий `zoom`.
#[derive(Default)]
pub struct ZoomScenario {
    run: Option<ZoomRun>,
}

impl ZoomScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservingScenario for ZoomScenario {
    fn name(&self) -> &'static str {
        "zoom"
    }

    /// Как `screen`: экран остаётся на месте и получает вход.
    fn camera_screen_policy(&self) -> CameraScreenPolicy {
        CameraScreenPolicy::KeepRunning
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let running = match context.camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let Some(injector) = context.camera_screen.input_injector() else {
            return ScenarioStep::Finished(failed(format!(
                "экран камеры не установлен (статус {:?}): переключать объективы нечем",
                context.camera_screen.status()
            )));
        };
        let config = context.camera.scenarios().zoom.clone();
        // Хвост событий прошлых сценариев и автозапуска к этому прогону не относится: дальше события забираем мы.
        let _stale = running.handle().drain_events(usize::MAX);
        let planned_steps = planned_steps(&config).len();
        context.emit_stage(
            "started",
            stage_fields([
                ("steps", planned_steps.into()),
                ("ready_timeout_ms", config.ready_timeout_ms.into()),
                ("step_timeout_ms", config.step_timeout_ms.into()),
            ]),
        );
        self.run = Some(ZoomRun {
            config,
            injector,
            phase: Phase::Preparing {
                started_at: context.now,
            },
            input_refusals: 0,
            planned_steps,
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
            return self.finish(context, Some(reason));
        }
        let engine = EngineView::of(&status);
        let presentation = match context.gpu.observe_camera_presentation() {
            CameraPresentationObservation::Live(presentation) => presentation,
            CameraPresentationObservation::RenderUnavailable { reason } => {
                return self.finish(context, Some(format!("рендер недоступен: {reason}")));
            }
            CameraPresentationObservation::NotShowingCamera => {
                return self.continue_or_time_out(context, "рендер не показывает камеру");
            }
        };
        let Some(viewfinder) = presentation.viewfinder.as_ref() else {
            return self.continue_or_time_out(context, "рендер без видоискателя: непрерывности кадров нет");
        };
        let Some(report) = context.camera_screen.report() else {
            return self.continue_or_time_out(context, "экран камеры ещё не нарисовал кадр");
        };
        let screen = ScreenView::of(&report);
        let frames = &viewfinder.continuity.recent_frames;
        let Some(render_now) = frames.last().map(|frame| frame.present_time) else {
            return self.continue_or_time_out(context, "рендер ещё не показал ни одного кадра");
        };
        let counters = &viewfinder.continuity.counters;
        let isp_counters = &viewfinder.counters;
        match &mut run.phase {
            Phase::Preparing { started_at } => {
                let started = *started_at;
                let engine_and_screen_ready = engine.streaming
                    && engine.catalog.back.is_some()
                    && engine.active.is_some()
                    && report.presence == camera_screen::CameraPresence::Live;
                if !engine_and_screen_ready {
                    return self.continue_or_time_out_since(context, started, "камера или экран ещё не готовы");
                }
                // Состояние движка не доказывает, что картинка уже на экране: касание в стартовый чёрный экран
                // некого удерживать и ломает вердикт. Ждём живой кадр активной камеры.
                let active_camera_on_screen = engine
                    .active
                    .as_ref()
                    .is_some_and(|active| screen_shows_live_picture_of(frames, &active.camera));
                if !active_camera_on_screen {
                    return self.continue_or_time_out_since(
                        context,
                        started,
                        "рендер ещё не показал живой кадр активной камеры",
                    );
                }
                let steps = planned_steps(&run.config);
                // Прогон привязан к «дому»: начинает с Back 1× и после провала возвращается туда же (`zoom_home`).
                let runner = ZoomRunner::new(run.config.clone(), steps).anchored_at_back_one_x();
                let baselines = Baselines {
                    continuity: counters.clone(),
                    switches_completed: status.lens.counters.switches_completed,
                    switches_failed: status.lens.counters.switches_failed,
                    switches_retargeted: status.lens.counters.switches_retargeted,
                    intents_ignored: status.lens.counters.intents_ignored,
                    lens_published: screen.lens_published,
                };
                run.phase = Phase::Stepping {
                    runner: Box::new(runner),
                    baselines: Box::new(baselines),
                };
                context.emit_stage("zoom_ready", stage_fields([("steps", run.planned_steps.into())]));
                ScenarioStep::Continue
            }
            Phase::Stepping { runner, .. } => {
                let observation = ZoomObservation {
                    now: context.now,
                    render_now,
                    engine: &engine,
                    events: &events,
                    frames,
                    counters,
                    isp_counters,
                    screen: &screen,
                };
                let tick = runner.tick(&observation);
                let surface = context.display.surface_size();
                for injection in tick.injections {
                    if deliver(&run.injector, injection, surface) != InputDelivery::Queued {
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

impl ZoomScenario {
    /// Ждать дальше, пока не вышел срок готовности; вышел — провал с причиной.
    fn continue_or_time_out(&mut self, context: &mut ScenarioContext<'_>, waiting_for: &str) -> ScenarioStep {
        let started = match self.run.as_ref().map(|run| &run.phase) {
            Some(Phase::Preparing { started_at }) => *started_at,
            // На шагах «нет кадров/экрана» — не ожидание готовности, а потеря наблюдения посреди прогона.
            Some(Phase::Stepping { .. }) => {
                return self.finish(
                    context,
                    Some(format!("посреди шагов пропало наблюдение: {waiting_for}")),
                );
            }
            None => return ScenarioStep::Continue,
        };
        self.continue_or_time_out_since(context, started, waiting_for)
    }

    fn continue_or_time_out_since(
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

    /// Завершение: собрать наблюдение, вывести подробности и вердикт.
    fn finish(&mut self, context: &mut ScenarioContext<'_>, run_failure: Option<String>) -> ScenarioStep {
        let Some(run) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let report = observation_of(run, context, run_failure);
        emit_details(context, "detail_zoom", report.detail_items());
        let config = context.camera.scenarios().zoom.clone();
        let violations = evaluate_zoom(&config, &report);
        ScenarioStep::Finished(finish_within_budget(context, report.summary(), violations))
    }
}

/// Отказ движка, который делает прогон бессмысленным: окончательная ошибка или отказ по существу.
fn engine_failure(status: &camera_core::CameraStatus, events: &[CameraEvent]) -> Option<String> {
    events
        .iter()
        .find_map(failure_of_event)
        .or_else(|| fatal_failure(status))
        .map(|failure| failure.describe())
}

/// Наблюдение на конец прогона.
fn observation_of(run: ZoomRun, context: &ScenarioContext<'_>, run_failure: Option<String>) -> ZoomObservationReport {
    let status = context.camera.running().ok().map(|running| running.handle().status());
    let report = context.camera_screen.report();
    let continuity = match context.gpu.observe_camera_presentation() {
        CameraPresentationObservation::Live(presentation) => {
            presentation.viewfinder.map(|viewfinder| viewfinder.continuity.counters)
        }
        _ => None,
    };
    let final_state = match (&status, &report) {
        (Some(status), Some(report)) => Some(FinalState::of(
            report.state.zoom,
            status.lens.active.as_ref().map(|active| active.role),
        )),
        _ => None,
    };
    let (steps, unfinished, totals) = match run.phase {
        Phase::Stepping { runner, baselines } => {
            let mut totals = ZoomTotals::default();
            if let (Some(end), Some(status), Some(report)) = (continuity.as_ref(), &status, &report) {
                let counters = &status.lens.counters;
                totals = ZoomTotals {
                    switches_completed: counters.switches_completed.saturating_sub(baselines.switches_completed),
                    switches_failed: counters.switches_failed.saturating_sub(baselines.switches_failed),
                    switches_retargeted: counters
                        .switches_retargeted
                        .saturating_sub(baselines.switches_retargeted),
                    intents_ignored: counters.intents_ignored.saturating_sub(baselines.intents_ignored),
                    lens_intents_published: report.effects.lens.published.saturating_sub(baselines.lens_published),
                    ..totals
                }
                .with_continuity(&baselines.continuity, end);
            }
            totals.input_refusals = run.input_refusals;
            (runner.records().to_vec(), runner.unfinished_names(), totals)
        }
        Phase::Preparing { .. } => (
            Vec::new(),
            planned_steps(&run.config).into_iter().map(|step| step.name).collect(),
            ZoomTotals {
                input_refusals: run.input_refusals,
                ..ZoomTotals::default()
            },
        ),
    };
    let run_failure = run_failure.or_else(|| {
        status.as_ref().and_then(|status| match status.state {
            CameraState::Error { .. } => Some("движок камеры в ошибке в конце прогона".to_owned()),
            _ => None,
        })
    });
    ZoomObservationReport {
        planned_steps: run.planned_steps,
        steps,
        unfinished,
        run_failure,
        totals,
        final_state,
    }
}

/// Вложение в экран: действие или образец касания (доли экрана переводятся в пиксели поверхности).
fn deliver(
    injector: &ScreenInputInjector,
    injection: Injection,
    surface: Option<shell_contract::surface::SurfaceSize>,
) -> InputDelivery {
    match injection {
        Injection::Action(action) => injector.inject(action),
        Injection::Touch(touch) => match surface {
            Some(size) => injector.inject_touch(sample_of(touch, size)),
            // Размера поверхности нет — некуда класть палец: честный отказ, не молчание.
            None => InputDelivery::ReceiverGone,
        },
    }
}

fn sample_of(touch: TouchInjection, size: shell_contract::surface::SurfaceSize) -> TouchSample {
    TouchSample {
        pointer: PointerId::new(0),
        phase: touch.phase,
        position: SurfacePoint {
            x_px: (touch.point.x * f64::from(size.width_px)) as f32,
            y_px: (touch.point.y * f64::from(size.height_px)) as f32,
        },
        time: MonotonicNanos::from_nanos(touch.time.as_nanos()),
    }
}
