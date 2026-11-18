//! Сценарий `3a` (сессия 08): автоматика 3A и ручные параметры на живом продуктовом видоискателе.
//!
//! Ход: общий запуск видоискателя ([`ViewfinderSession`], как у `viewfinder`), затем шаги по порядку
//! (`three_a_steps*`): a живая автоматика HAL, b готовность AWB (стадия `awb_ready` — на ней драйвер может
//! снять скриншот), c тапы фокуса, d ручная экспозиция, e компенсация EV, f блокировка AE/AF, g брекетинг.
//! Всё по тикам, ничего не блокируется. В конце — возврат автоматических параметров, подробности
//! `detail_three_a` и результат с вердиктом [`evaluate_three_a`].
//!
//! Страховка возврата двойная: сценарий сам шлёт «всё автоматическое, вся рамка», а маршрутизатор при любом
//! конце закрывает камеру сценария (`scenario_ended`) — `Close` забывает желаемое состояние движка.

use std::collections::VecDeque;

use camera_contract::control::ManualControl;
use camera_core::CameraEvent;

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::result_budget::finish_within_budget;
use super::three_a_own_ae::OwnAeWatch;
use super::three_a_report::ThreeAObservation;
use super::three_a_results::ResultLog;
use super::three_a_steps::{StartBaselines, StepEnv, StepProgress, ThreeAStep};
use super::three_a_steps_bracket::BracketStep;
use super::three_a_steps_exposure::{EvStep, ManualStep};
use super::three_a_steps_focus::{AwbStep, LiveStep, LockStep, TapStep};
use super::three_a_verdict::{evaluate_three_a, live_violations};
use super::viewfinder_session::{SessionStage, SessionTick, SessionTiming, ViewfinderSession};
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::three_a_scenario_config::ThreeAScenarioConfig;
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Состояние прогона.
struct ThreeARun {
    config: ThreeAScenarioConfig,
    session: ViewfinderSession,
    results: ResultLog,
    pending_steps: VecDeque<Box<dyn ThreeAStep>>,
    current: Option<Box<dyn ThreeAStep>>,
    baselines: Option<StartBaselines>,
    /// Наблюдение за нашим AE (пусто, пока экспозицию ведёт вендорский).
    own_ae: OwnAeWatch,
    observation: ThreeAObservation,
}

/// Порядок шагов сценария.
fn all_steps() -> VecDeque<Box<dyn ThreeAStep>> {
    VecDeque::from([
        Box::new(LiveStep::default()) as Box<dyn ThreeAStep>,
        Box::new(AwbStep::default()),
        Box::new(TapStep::default()),
        Box::new(ManualStep::default()),
        Box::new(EvStep::default()),
        Box::new(LockStep::default()),
        Box::new(BracketStep::default()),
    ])
}

/// Сценарий `3a`.
#[derive(Default)]
pub struct ThreeAScenario {
    run: Option<ThreeARun>,
}

impl ThreeAScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Что делать после тика.
enum RunStep {
    Continue,
    Finish(Option<RunFailure>),
}

impl ThreeARun {
    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        let mut stages = self.session.take_stages();
        let step = match tick {
            SessionTick::Preparing => RunStep::Continue,
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure)),
            SessionTick::BecameLive(presentation) => {
                self.baselines = Some(start_baselines(context, &presentation));
                self.drive(context, running, &presentation, &mut stages)
            }
            SessionTick::Live(presentation) => self.drive(context, running, &presentation, &mut stages),
        };
        for stage in stages {
            context.emit_stage(stage.stage, stage.fields);
        }
        step
    }

    /// Один тик шагов: журнал результатов, затем текущий шаг (или переход к следующему).
    fn drive(
        &mut self,
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        presentation: &CameraPresentation,
        stages: &mut Vec<SessionStage>,
    ) -> RunStep {
        let Some(baselines) = self.baselines else {
            return RunStep::Continue;
        };
        let status = running.handle().status();
        self.results.observe(status.three_a.last_result.as_ref());
        self.own_ae.observe(&status.three_a);
        let events: Vec<CameraEvent> = self.session.three_a_events().to_vec();
        let mut env = StepEnv {
            now: context.now,
            running,
            status: &status,
            events: &events,
            presentation,
            results: &self.results,
            config: &self.config,
            baselines,
            raw_counters: context.camera.raw_stream().counters(),
            stages,
        };
        loop {
            let begun_now = self.current.is_none();
            if begun_now {
                let Some(mut next) = self.pending_steps.pop_front() else {
                    return RunStep::Finish(None);
                };
                if let Err(failure) = next.begin(&mut env, &mut self.observation) {
                    return RunStep::Finish(Some(failure));
                }
                self.current = Some(next);
            }
            let Some(step) = self.current.as_mut() else {
                return RunStep::Continue;
            };
            match step.poll(&mut env, &mut self.observation) {
                StepProgress::Pending => return RunStep::Continue,
                StepProgress::Failed(failure) => return RunStep::Finish(Some(failure)),
                StepProgress::Done => {
                    let finished = step.name();
                    self.current = None;
                    if finished == "live" && self.live_is_dead() {
                        // Автоматика HAL мертва: остальные шаги только ждали бы таймауты.
                        self.pending_steps.clear();
                    }
                }
            }
        }
    }

    fn live_is_dead(&self) -> bool {
        self.observation
            .live
            .as_ref()
            .is_some_and(|live| !live_violations(live).is_empty())
    }
}

/// Возврат автоматики: всё автоматическое, вся рамка. Отказ очереди — причина для вердикта, не тишина.
fn restore_automatic(running: &RunningCamera) -> Option<String> {
    let handle = running.handle();
    let controls = handle
        .apply_controls(ManualControl::AUTOMATIC)
        .err()
        .map(|rejected| format!("apply_controls: {rejected}"));
    let point = handle
        .reset_point_of_interest()
        .err()
        .map(|rejected| format!("reset_point_of_interest: {rejected}"));
    match (controls, point) {
        (None, None) => None,
        (first, second) => Some([first, second].into_iter().flatten().collect::<Vec<_>>().join("; ")),
    }
}

/// Счётчики AWB на начало сценария (отсчёт для «скрыто с начала»).
fn start_baselines(context: &ScenarioContext<'_>, presentation: &CameraPresentation) -> StartBaselines {
    let diagnostics = presentation
        .viewfinder
        .as_ref()
        .map(|viewfinder| &viewfinder.white_balance);
    StartBaselines {
        hidden_frames: diagnostics.map_or(0, |wb| wb.hidden_frames_cold_start),
        forced_shows: diagnostics.map_or(0, |wb| wb.cold_start_forced),
        max_hidden_frames: context.gpu.render_config.viewfinder.white_balance.max_hidden_frames,
    }
}

impl ObservingScenario for ThreeAScenario {
    fn name(&self) -> &'static str {
        "3a"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        // Режим и сроки запуска — как у `viewfinder`; свои — только проверки автоматики.
        let viewfinder: ViewfinderScenarioConfig = camera.scenarios().viewfinder.clone();
        let step = match viewfinder.product_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&viewfinder)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        let config = camera.scenarios().three_a.clone();
        context.emit_stage(
            "started",
            stage_fields([
                ("tap_points", config.tap.points.len().into()),
                ("bracket_frames", config.bracket.exposures_ns.len().into()),
            ]),
        );
        let mut observation = ThreeAObservation::new(session.summary(running, EVENTS_IN_REPORT_LIMIT));
        // Кого композиция поставила вести экспозицию: расхождение с движком — нарушение вердикта.
        let choice = camera.exposure_controller();
        observation.expected_ae_source = choice.effective.into();
        observation.ae_fallback_reason = choice.fallback_reason.clone();
        self.run = Some(ThreeARun {
            config,
            session,
            results: ResultLog::default(),
            pending_steps: all_steps(),
            current: None,
            baselines: None,
            own_ae: OwnAeWatch::default(),
            observation,
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
        let RunStep::Finish(failure) = run.tick(context, running) else {
            return ScenarioStep::Continue;
        };
        let Some(mut finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        // Возврат автоматики при любом конце, в том числе провальном.
        finished.observation.restore_failure = restore_automatic(running);
        finished.observation.failure = failure;
        finished.observation.own_ae = finished.own_ae.report(&finished.results);
        finished.observation.session = finished.session.summary(running, EVENTS_IN_REPORT_LIMIT);
        emit_details(context, "detail_three_a", finished.observation.detail_items());
        let violations = evaluate_three_a(&finished.config, &finished.observation);
        let summary = finished.observation.summary();
        ScenarioStep::Finished(finish_within_budget(context, summary, violations))
    }
}
