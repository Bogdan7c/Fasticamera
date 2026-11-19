//! Сценарий `ae-trace` (сессия 08c, шаги 5–6): ряд нашего AE по ступенькам света, подтяжка ISP, мерцание.
//!
//! Владелец сидит у телефона с лампой. Телефон показывает на экране подсказки (отсчёт «приготовься… 3, 2, 1», затем «щёлкни лампой»), владелец включает или гасит лампу.
//! Сценарий пишет ряд (`ae_trace_series`), по расписанию (`ae_trace_plan`) переключает профиль 3A и показывает подсказки
//! (`ae_trace_run`, тексты — `ae_trace_prompts`), в конце разбирает ряд (`ae_trace_analysis`), судит жёсткие нарушения (`ae_trace_verdict`) и
//! выводит сводку и подробности (`ae_trace_report`).
//!
//! Ход: общий запуск видоискателя ([`ViewfinderSession`], как у `3a`) -> ожидание первой диагностики нашего AE ->
//! три фазы (`photo`, `video`, `static`). Всё по тикам, ничего не блокируется.
//!
//! Страховка возврата двойная, как у `3a`: сценарий сам возвращает профиль «Фото», а маршрутизатор при любом конце
//! закрывает камеру сценария (`scenario_ended`).

use camera_contract::three_a::ThreeAProfile;

use super::ae_trace_analysis::analyze_run;
use super::ae_trace_observation::{AeTraceObservation, StartTimings};
use super::ae_trace_report::{detail_items, summary};
use super::ae_trace_run::{Advance, Readings, RecordingServices, Tracing};
use super::ae_trace_verdict::evaluate_ae_trace;
use super::detail::emit_details;
use super::progress::Deadline;
use super::result_budget::finish_within_budget;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use crate::ae_trace_scenario_config::AeTraceScenarioConfig;
use crate::auto_exposure_link::ExposureControllerKind;
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Состояние прогона.
struct AeTraceRun {
    config: AeTraceScenarioConfig,
    session: ViewfinderSession,
    /// Срок ожидания первой диагностики AE; взводится, когда видоискатель стал живым.
    ae_deadline: Option<Deadline>,
    /// Запись; `None`, пока диагностики AE не было.
    tracing: Option<Tracing>,
    /// Предел подтяжки ISP контроллера, EV.
    gain_limit_ev: f64,
}

/// Что делать после тика.
enum RunStep {
    Continue,
    /// Конец; `Some` — причина провала стенда или камеры.
    Finish(Option<String>),
}

impl AeTraceRun {
    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        let stages = self.session.take_stages();
        let step = match tick {
            SessionTick::Preparing => RunStep::Continue,
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure.describe())),
            SessionTick::BecameLive(presentation) => {
                self.ae_deadline = Some(Deadline::after_millis(
                    context.now,
                    self.config.ae_diagnostics_timeout_ms,
                ));
                self.drive(context, running, &presentation)
            }
            SessionTick::Live(presentation) => self.drive(context, running, &presentation),
        };
        for stage in stages {
            context.emit_stage(stage.stage, stage.fields);
        }
        step
    }

    /// Один тик живого видоискателя: ждём диагностику AE или двигаем запись.
    fn drive(
        &mut self,
        context: &mut ScenarioContext<'_>,
        running: &RunningCamera,
        presentation: &CameraPresentation,
    ) -> RunStep {
        let diagnostics = presentation
            .viewfinder
            .as_ref()
            .and_then(|viewfinder| viewfinder.auto_exposure.as_ref());
        let Some(diagnostics) = diagnostics else {
            return self.on_missing_diagnostics(context);
        };
        let tracing = self.tracing.get_or_insert_with(|| {
            context.emit_stage(
                "trace_started",
                stage_fields([("total_ms", self.config.total_ms().into())]),
            );
            Tracing::begin(&self.config, context.now)
        });
        let status = running.handle().status();
        let advance = tracing.advance(
            &self.config,
            context.now,
            RecordingServices {
                handle: running.handle(),
                prompts: context.services.operator_prompt.as_ref(),
            },
            Readings {
                diagnostics,
                three_a: &status.three_a,
            },
        );
        match advance {
            Ok(Advance::Continue) => RunStep::Continue,
            Ok(Advance::ScheduleFinished) => RunStep::Finish(None),
            Err(failure) => RunStep::Finish(Some(failure.describe())),
        }
    }

    /// Диагностики AE нет: до начала записи ждём срок, после — просто считаем пропуск.
    fn on_missing_diagnostics(&mut self, context: &ScenarioContext<'_>) -> RunStep {
        if let Some(tracing) = self.tracing.as_mut() {
            return match tracing.note_missing_diagnostics(context.now) {
                Advance::Continue => RunStep::Continue,
                Advance::ScheduleFinished => RunStep::Finish(None),
            };
        }
        let expired = self
            .ae_deadline
            .is_some_and(|deadline| deadline.is_expired(context.now));
        if expired {
            RunStep::Finish(Some(no_diagnostics_reason(context)))
        } else {
            RunStep::Continue
        }
    }
}

/// Почему диагностики нашего AE нет: объяснение для нарушения.
fn no_diagnostics_reason(context: &ScenarioContext<'_>) -> String {
    let choice = context.camera.exposure_controller();
    match (&choice.effective, &choice.fallback_reason) {
        (ExposureControllerKind::Vendor, Some(reason)) => {
            format!("экспозицию ведёт вендорский AE (замена нашего: {reason}): ряд нашего AE записать нечем")
        }
        (ExposureControllerKind::Vendor, None) => {
            "экспозицию ведёт вендорский AE (так настроено): ряд нашего AE записать нечем".to_owned()
        }
        (ExposureControllerKind::Own, _) => {
            "наш AE выбран, но рендер не публикует его диагностику за отведённый срок".to_owned()
        }
    }
}

/// Возврат профиля «Фото» (профиль по умолчанию). Отказ очереди — причина для вердикта, не тишина.
fn restore_photo_profile(running: &RunningCamera) -> Option<String> {
    running
        .handle()
        .set_three_a_profile(ThreeAProfile::Photo)
        .err()
        .map(|rejected| format!("set_three_a_profile: {rejected}"))
}

/// Сценарий `ae-trace`.
#[derive(Default)]
pub struct AeTraceScenario {
    run: Option<AeTraceRun>,
}

impl AeTraceScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ObservingScenario for AeTraceScenario {
    fn name(&self) -> &'static str {
        "ae-trace"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        // Вендорский AE не даёт ряда нашего: отказываем сразу, до заявки камеры, с причиной замены.
        if camera.exposure_controller().effective == ExposureControllerKind::Vendor {
            return ScenarioStep::Finished(failed(no_diagnostics_reason(context)));
        }
        // Режим и сроки запуска — как у `viewfinder`; свои — только расписание и пороги ряда.
        let viewfinder: ViewfinderScenarioConfig = camera.scenarios().viewfinder.clone();
        let step = match viewfinder.product_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&viewfinder)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        let config = camera.scenarios().ae_trace.clone();
        context.emit_stage("started", stage_fields([("total_ms", config.total_ms().into())]));
        let gain_limit_ev = f64::from(
            context
                .gpu
                .render_config
                .viewfinder
                .auto_exposure
                .controller
                .max_residual_ev,
        );
        self.run = Some(AeTraceRun {
            config,
            session,
            ae_deadline: None,
            tracing: None,
            gain_limit_ev,
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
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let observation = collect_observation(finished, failure, running);
        emit_details(
            context,
            "detail_ae_trace",
            detail_items(&camera.scenarios().ae_trace, &observation),
        );
        let violations = evaluate_ae_trace(&camera.scenarios().ae_trace, &observation);
        ScenarioStep::Finished(finish_within_budget(context, summary(&observation), violations))
    }
}

/// Собирает наблюдения конца прогона: возвращает профиль, разбирает ряд (если расписание дошло до конца).
fn collect_observation(run: AeTraceRun, failure: Option<String>, running: &RunningCamera) -> AeTraceObservation {
    let session = run.session.summary(running, 0);
    let mut observation = AeTraceObservation {
        failure,
        // Возврат профиля при любом конце, в том числе провальном.
        restore_failure: restore_photo_profile(running),
        gain_limit_ev: run.gain_limit_ev,
        timings: StartTimings {
            time_to_streaming_ms: session.time_to_streaming_ms,
            time_to_live_ms: session.time_to_live_ms,
        },
        ..AeTraceObservation::default()
    };
    if let Some(tracing) = run.tracing {
        let outcome = tracing.finish();
        observation.dropped_samples = outcome.dropped_samples;
        observation.ticks_without_ae = outcome.ticks_without_ae;
        observation.prompts = outcome.prompts;
        // Разбираем только полный прогон: по обрубку числа ступенек были бы выдумкой.
        if observation.failure.is_none() {
            observation.analysis = Some(analyze_run(&run.config, &outcome.timeline, &outcome.series));
        }
        observation.timeline = Some(outcome.timeline);
        observation.series = outcome.series;
    }
    observation
}
