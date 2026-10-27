//! Сценарий `viewfinder-endurance` (сессия 07): 10 минут продуктового видоискателя с ISP.
//!
//! Ход: общий запуск, затем поминутная запись (`power.endurance`: длительность и шаг): ток и заряд
//! батареи, тепловой статус и headroom, частота сенсора по `SENSOR_TIMESTAMP`, медиана/p95 прохода
//! ISP на GPU. В конце — признаки троттлинга по порогам `viewfinder.endurance`
//! ([`find_throttling`]) и вердикт годности прогона ([`evaluate_endurance`]).

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::result_budget::finish_within_budget;
use super::viewfinder_endurance_report::{ViewfinderEnduranceObservation, evaluate_endurance, find_throttling};
use super::viewfinder_metrics::{WindowBaseline, WindowDelta, millis};
use super::viewfinder_minutes::MinuteLog;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::power_measurement::endurance::IspGpuTime;
use crate::power_measurement::{EnduranceStep, MinuteObservation, PowerSettings, ThermalSnapshot};
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Бюджет сводки прогона в строке результата, байт (как у `raw-stream`: запись logcat 4000 байт).
pub(super) const ENDURANCE_SUMMARY_BUDGET_BYTES: usize = 2_000;

/// Что сценарий видит сам в конце минуты.
pub fn minute_observation(
    context: &ScenarioContext<'_>,
    running: &RunningCamera,
    presentation: Option<&CameraPresentation>,
) -> MinuteObservation {
    let isp_gpu_time = presentation
        .and_then(|presentation| presentation.viewfinder.as_ref())
        .and_then(|viewfinder| viewfinder.gpu_time.isp_dispatch)
        .map(|summary| IspGpuTime {
            median_ms: millis(summary.p50),
            p95_ms: millis(summary.p95),
        });
    MinuteObservation {
        thermal: ThermalSnapshot::read(context.services.thermal.as_ref()),
        sensor_fps: running.handle().status().stream.measured_frames_per_second,
        isp_gpu_time,
    }
}

struct EnduranceRun {
    config: ViewfinderScenarioConfig,
    power: PowerSettings,
    session: ViewfinderSession,
    minutes: Option<MinuteLog>,
    baseline: Option<WindowBaseline>,
    last: Option<Box<CameraPresentation>>,
}

/// Сценарий `viewfinder-endurance`.
#[derive(Default)]
pub struct ViewfinderEnduranceScenario {
    run: Option<EnduranceRun>,
}

impl ViewfinderEnduranceScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

enum RunStep {
    Continue,
    Finish(Option<RunFailure>, Option<String>),
}

impl EnduranceRun {
    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        let presentation = match tick {
            SessionTick::Preparing => return RunStep::Continue,
            SessionTick::Failed(failure) => return RunStep::Finish(Some(failure), None),
            SessionTick::BecameLive(presentation) => {
                self.baseline = Some(WindowBaseline::capture(
                    context.now,
                    &presentation,
                    context.camera.raw_stream().counters(),
                ));
                match MinuteLog::start(context, &self.power) {
                    Ok(minutes) => self.minutes = Some(minutes),
                    Err(error) => return RunStep::Finish(None, Some(error)),
                }
                presentation
            }
            SessionTick::Live(presentation) => presentation,
        };
        self.last = Some(presentation);
        let Some(minutes) = &mut self.minutes else {
            return RunStep::Continue;
        };
        let last = self.last.as_deref();
        let mut closed = Vec::new();
        let step = minutes.tick(|| {
            let observation = minute_observation(context, running, last);
            closed.push(observation.clone());
            observation
        });
        for observation in closed {
            context.emit_stage(
                "minute",
                stage_fields([
                    ("thermal", observation.thermal.status.map(|status| status.name()).into()),
                    ("headroom", observation.thermal.headroom.into()),
                    ("sensor_fps", observation.sensor_fps.into()),
                    (
                        "isp_median_ms",
                        observation.isp_gpu_time.map(|time| time.median_ms).into(),
                    ),
                ]),
            );
        }
        match step {
            Ok(EnduranceStep::Recording { .. }) => RunStep::Continue,
            Ok(EnduranceStep::Finished) => RunStep::Finish(None, None),
            Err(error) => RunStep::Finish(None, Some(error)),
        }
    }

    fn into_observation(
        self,
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        failure: Option<RunFailure>,
    ) -> ViewfinderEnduranceObservation {
        let window = match (&self.baseline, &self.last) {
            (Some(baseline), Some(last)) => {
                WindowDelta::since(baseline, context.now, last, &context.camera.raw_stream().counters())
            }
            _ => WindowDelta::default(),
        };
        let (report, expected_minutes, power_unavailable) = match &self.minutes {
            Some(minutes) => (
                minutes.recorder().report(),
                minutes.total_minutes(),
                minutes.power_unavailable().map(str::to_owned),
            ),
            None => {
                // Запись минут не началась (отказ до живого видоискателя): пустой отчёт с честным
                // числом ожидаемых минут — точка отсчёта времени здесь ни на что не влияет.
                let recorder = crate::power_measurement::EnduranceRecorder::start(
                    &self.power,
                    shell_contract::clock::MonotonicNanos::from_nanos(0),
                    ThermalSnapshot::read(context.services.thermal.as_ref()),
                );
                (recorder.report(), recorder.total_minutes(), None)
            }
        };
        ViewfinderEnduranceObservation {
            failure,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            expected_minutes,
            power_unavailable,
            throttling: find_throttling(&self.config.endurance, &report),
            report,
            window,
        }
    }
}

impl ObservingScenario for ViewfinderEnduranceScenario {
    fn name(&self) -> &'static str {
        "viewfinder-endurance"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = camera.scenarios().viewfinder.clone();
        let power = match camera.scenarios().power.settings() {
            Ok(power) => power,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.power: {error}"))),
        };
        let step = match config.product_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&config)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        context.emit_stage(
            "started",
            stage_fields([(
                "duration_ms",
                u64::try_from(power.endurance.duration.as_millis())
                    .unwrap_or(u64::MAX)
                    .into(),
            )]),
        );
        self.run = Some(EnduranceRun {
            config,
            power,
            session,
            minutes: None,
            baseline: None,
            last: None,
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
        let RunStep::Finish(failure, clock_error) = run.tick(context, running) else {
            return ScenarioStep::Continue;
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        if let Some(error) = clock_error {
            return ScenarioStep::Finished(failed(error));
        }
        let observation = finished.into_observation(context, running, failure);
        emit_details(context, "detail_endurance", observation.detail_items());
        let violations = evaluate_endurance(&observation);
        ScenarioStep::Finished(finish_within_budget(
            context,
            observation.summary(ENDURANCE_SUMMARY_BUDGET_BYTES),
            violations,
        ))
    }
}
