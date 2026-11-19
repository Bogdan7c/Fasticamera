//! Сценарий `viewfinder` (сессия 07): минута продуктового видоискателя с ISP на GPU.
//!
//! Ход: общий запуск ([`ViewfinderSession`]: камера в продуктовом режиме 4:3@60 RAW10 → `Streaming`
//! → первый кадр на экране → установление AE, стадия `viewfinder_live` — на ней скрипт снимает
//! скриншоты), затем `smoke.measure_ms` замера: на каждом тике копится задержка «сенсор → экран»,
//! раз в `progress_interval_ms` — стадия `progress`. В конце — `detail_viewfinder` и результат с
//! вердиктом [`evaluate_viewfinder`]. Рендер сценарий не регулирует: меряется продукт как есть.

use std::time::{Duration, Instant};

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::raw_stream_samples::LatencySampler;
use super::result_budget::finish_within_budget;
use super::viewfinder_metrics::{GpuTimeMs, WindowBaseline, WindowDelta, collect_latency, round2};
use super::viewfinder_pacing_report::PacingWindowReport;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use super::viewfinder_smoke_report::{
    ViewfinderGpuTimesMs, ViewfinderLook, ViewfinderSmokeObservation, evaluate_viewfinder, viewfinder_summary,
};
use super::viewfinder_tuning_report::{AdpfReport, PresentTargetReport};
use super::white_balance_report::WhiteBalanceReport;
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::power_measurement::ThermalSnapshot;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Замер после установления видоискателя.
struct Measuring {
    baseline: WindowBaseline,
    until: Instant,
    next_progress: Instant,
}

/// Состояние прогона. Общее с сценарием `glass` (он ведёт тот же замер видоискателя и добавляет своё).
pub(super) struct SmokeRun {
    config: ViewfinderScenarioConfig,
    session: ViewfinderSession,
    measuring: Option<Measuring>,
    latency: LatencySampler,
    thermal_start: ThermalSnapshot,
    last: Option<Box<CameraPresentation>>,
}

/// Сценарий `viewfinder`.
#[derive(Default)]
pub struct ViewfinderSmokeScenario {
    run: Option<SmokeRun>,
}

impl ViewfinderSmokeScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Что сделать после тика.
pub(super) enum RunStep {
    Continue,
    Finish(Option<RunFailure>),
}

/// Почему окно замера не удалось начать заново.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(super) enum WindowRestartError {
    /// Видоискатель ещё не установился: замер не начинался, перезапускать нечего.
    #[error("окно замера ещё не начиналось: видоискатель не установился")]
    NotMeasuring,
}

impl SmokeRun {
    /// Заявляет камеру, отправляет команды запуска и готовит замер (стадия `started` — за вызывающим).
    /// Ошибка — готовый текст провала сценария.
    pub(super) fn begin(
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        config: ViewfinderScenarioConfig,
    ) -> Result<Self, String> {
        let step = config
            .product_step()
            .map_err(|error| format!("camera.scenarios.viewfinder: {error}"))?;
        let session = ViewfinderSession::start(context, running, &step, SessionTiming::of(&config))
            .map_err(|failure| failure.describe())?;
        Ok(Self {
            latency: LatencySampler::new(config.latency_samples_limit, config.latency_plausible_max_ms),
            thermal_start: ThermalSnapshot::read(context.services.thermal.as_ref()),
            config,
            session,
            measuring: None,
            last: None,
        })
    }

    /// Идёт ли замер (видоискатель установился). Становится истиной на тике `BecameLive`, один раз навсегда.
    pub(super) fn is_measuring(&self) -> bool {
        self.measuring.is_some()
    }

    /// Последний снимок показа (`None` — рендер ещё не показывал камеру нашей сессии).
    pub(super) fn last_presentation(&self) -> Option<&CameraPresentation> {
        self.last.as_deref()
    }

    /// Заново начинает окно замера с текущего момента: базовая линия, срок окончания (`measure_ms` от
    /// сейчас), прогресс и задержки считаются заново, уже показанное в замер не входит. Нужно сценарию,
    /// который после старта окна добавляет в кадр что-то дорогое в первые кадры (прогрев слоя UI в `glass`)
    /// и не хочет, чтобы этот разгон попал в вердикт. Сценарий `viewfinder` метод не вызывает.
    pub(super) fn restart_window(&mut self, context: &ScenarioContext<'_>) -> Result<(), WindowRestartError> {
        let (Some(measuring), Some(last)) = (&mut self.measuring, &self.last) else {
            return Err(WindowRestartError::NotMeasuring);
        };
        let now = context.now;
        self.latency.skip_already_presented(&last.diagnostics.present_history);
        measuring.baseline = WindowBaseline::capture(now, last, context.camera.raw_stream().counters());
        measuring.until = now + Duration::from_millis(self.config.smoke.measure_ms);
        measuring.next_progress = now + Duration::from_millis(self.config.progress_interval_ms);
        Ok(())
    }

    pub(super) fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        match tick {
            SessionTick::Preparing => RunStep::Continue,
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure)),
            SessionTick::BecameLive(presentation) => {
                let now = context.now;
                // Задержка меряется по установившемуся потоку: кадры периода сходимости AE не в счёт.
                self.latency
                    .skip_already_presented(&presentation.diagnostics.present_history);
                self.measuring = Some(Measuring {
                    baseline: WindowBaseline::capture(now, &presentation, context.camera.raw_stream().counters()),
                    until: now + Duration::from_millis(self.config.smoke.measure_ms),
                    next_progress: now + Duration::from_millis(self.config.progress_interval_ms),
                });
                self.last = Some(presentation);
                RunStep::Continue
            }
            SessionTick::Live(presentation) => {
                collect_latency(&mut self.latency, context.camera.raw_stream(), &presentation);
                self.last = Some(presentation);
                self.progress_and_deadline(context)
            }
        }
    }

    fn progress_and_deadline(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        let now = context.now;
        let (Some(measuring), Some(last)) = (&mut self.measuring, &self.last) else {
            return RunStep::Continue;
        };
        if now >= measuring.next_progress {
            measuring.next_progress = now + Duration::from_millis(self.config.progress_interval_ms);
            let delta = WindowDelta::since(&measuring.baseline, now, last, &context.camera.raw_stream().counters());
            let isp_p95 = last
                .viewfinder
                .as_ref()
                .and_then(|viewfinder| viewfinder.gpu_time.isp_dispatch.as_ref())
                .map(|summary| GpuTimeMs::of(summary).p95_ms);
            let thermal = ThermalSnapshot::read(context.services.thermal.as_ref());
            context.emit_stage(
                "progress",
                stage_fields([
                    ("elapsed_ms", delta.window_ms.into()),
                    ("isp_fps", delta.isp_frames_per_second().map(round2).into()),
                    ("frames_presented", delta.camera_frames_presented.into()),
                    ("isp_dispatch_p95_ms", isp_p95.into()),
                    ("thermal", thermal.status.map(|status| status.name()).into()),
                ]),
            );
        }
        if now >= measuring.until {
            RunStep::Finish(None)
        } else {
            RunStep::Continue
        }
    }

    pub(super) fn into_observation(
        self,
        context: &ScenarioContext<'_>,
        running: &RunningCamera,
        failure: Option<RunFailure>,
    ) -> ViewfinderSmokeObservation {
        let status = running.handle().status();
        let window = match (&self.measuring, &self.last) {
            (Some(measuring), Some(last)) => WindowDelta::since(
                &measuring.baseline,
                context.now,
                last,
                &context.camera.raw_stream().counters(),
            ),
            _ => WindowDelta::default(),
        };
        let pacing = match (&self.measuring, &self.last) {
            (Some(measuring), Some(last)) => {
                measuring
                    .baseline
                    .pacing_since(context.now, last, &context.camera.raw_stream().counters())
            }
            _ => PacingWindowReport::default(),
        };
        let viewfinder = self.last.as_ref().and_then(|last| last.viewfinder.as_ref());
        ViewfinderSmokeObservation {
            failure,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            sensor_fps: status.stream.measured_frames_per_second,
            window,
            pacing,
            gpu_time: viewfinder.map_or_else(Default::default, |viewfinder| {
                ViewfinderGpuTimesMs::of(&viewfinder.gpu_time)
            }),
            look: viewfinder.map(ViewfinderLook::of),
            validation: self.last.as_ref().map(|last| last.validation.clone()),
            latency: self.latency.report(),
            present_target: self
                .last
                .as_ref()
                .map(|last| PresentTargetReport::of(&last.present_target)),
            adpf: self.last.as_ref().map(|last| AdpfReport::of(&last.adpf)),
            acquire_wait: self
                .last
                .as_ref()
                .and_then(|last| GpuTimeMs::of_optional(last.present_target.acquire_wait.as_ref())),
            latch_to_submit: self
                .last
                .as_ref()
                .and_then(|last| GpuTimeMs::of_optional(last.present_target.latch_to_submit.as_ref())),
            thermal_start: self.thermal_start,
            thermal_end: ThermalSnapshot::read(context.services.thermal.as_ref()),
            last_isp_failure: viewfinder.and_then(|viewfinder| viewfinder.counters.last_isp_failure.clone()),
            last_params_failure: viewfinder.and_then(|viewfinder| viewfinder.counters.last_params_failure.clone()),
            white_balance: viewfinder.map(|viewfinder| WhiteBalanceReport::of(&viewfinder.white_balance)),
            companion: running.diagnostics_snapshot().three_a_companion,
        }
    }
}

impl ObservingScenario for ViewfinderSmokeScenario {
    fn name(&self) -> &'static str {
        "viewfinder"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = camera.scenarios().viewfinder.clone();
        let run = match SmokeRun::begin(context, running, config.clone()) {
            Ok(run) => run,
            Err(reason) => return ScenarioStep::Finished(failed(reason)),
        };
        context.emit_stage(
            "started",
            stage_fields([
                ("measure_ms", config.smoke.measure_ms.into()),
                ("isp_dispatch_budget_ms", config.smoke.isp_dispatch_budget_ms.into()),
            ]),
        );
        self.run = Some(run);
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
        let smoke_config = finished.config.smoke.clone();
        let observation = finished.into_observation(context, running, failure);
        emit_details(context, "detail_viewfinder", observation.detail_items());
        let violations = evaluate_viewfinder(&smoke_config, &observation);
        let summary = viewfinder_summary(&smoke_config, &observation);
        ScenarioStep::Finished(finish_within_budget(context, summary, violations))
    }
}
