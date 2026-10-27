//! Сценарий `video-headroom-bench` (V4, сессия 07): стенд запаса под видео.
//!
//! Ход: камера в видео-режиме (`viewfinder.headroom`: основная 16:9@60, 4096×2304) через общий
//! запуск; когда видоискатель установился — просьба к платформе о 120 Гц и включение стенда
//! регулировкой видоискателя (стадия `bench_started`: на ней скрипт касается экрана — OxygenOS держит
//! приложению 60 Гц до первого касания, бэклог п. 1). Дальше поминутно: тепло, ток, время кадра
//! стенда, достигнутая частота экрана. В конце — снимок стенда (время по проходам, весь кадр против
//! бюджета 16,7 мс) и вердикт годности прогона ([`evaluate_headroom`]).
//!
//! Стенд выключает, 60 Гц и продуктовый видоискатель возвращает маршрутизатор по концу сценария
//! (`GpuServices::render_scenario_ended`, `CameraServices::scenario_ended`) — при любом исходе.

use render_loop::viewfinder::HeadroomBench;
use shell_contract::commands::FrameRateOutcome;

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::result_budget::finish_within_budget;
use super::video_headroom_report::{
    BenchMinute, BenchSnapshot, DisplayDuringBench, HeadroomObservation, evaluate_headroom, fits_frame_budget,
};
use super::viewfinder_endurance::minute_observation;
use super::viewfinder_metrics::GpuTimeMs;
use super::viewfinder_minutes::MinuteLog;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::power_measurement::config::EnduranceSettings;
use crate::power_measurement::{EnduranceRecorder, EnduranceStep, PowerSettings, ThermalSnapshot};
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_scenario_config::ViewfinderScenarioConfig;

/// Настройки тока с длительностью и шагом стенда (а не 10-минутного прогона видоискателя).
fn bench_power_settings(power: &PowerSettings, config: &ViewfinderScenarioConfig) -> PowerSettings {
    PowerSettings {
        endurance: EnduranceSettings {
            duration: std::time::Duration::from_millis(config.headroom.duration_ms),
            snapshot_interval: std::time::Duration::from_millis(config.headroom.snapshot_interval_ms),
        },
        ..*power
    }
}

struct HeadroomRun {
    config: ViewfinderScenarioConfig,
    power: PowerSettings,
    session: ViewfinderSession,
    minutes: Option<MinuteLog>,
    bench_minutes: Vec<BenchMinute>,
    frames_at_start: u64,
    refresh_request: Option<FrameRateOutcome>,
    undelivered: Vec<String>,
    last: Option<Box<CameraPresentation>>,
}

/// Сценарий `video-headroom-bench`.
#[derive(Default)]
pub struct VideoHeadroomScenario {
    run: Option<HeadroomRun>,
}

impl VideoHeadroomScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

enum RunStep {
    Continue,
    Finish(Option<RunFailure>, Option<String>),
}

/// Время кадра стенда и частота экрана из снимка показа.
fn bench_minute(minute: u32, presentation: Option<&CameraPresentation>) -> BenchMinute {
    BenchMinute {
        minute,
        total: presentation
            .and_then(|presentation| presentation.viewfinder.as_ref())
            .and_then(|viewfinder| viewfinder.headroom.total.as_ref())
            .map(GpuTimeMs::of),
        refresh_hz: DisplayDuringBench::refresh_hz_of(
            presentation.and_then(|presentation| presentation.display_refresh_cycle),
        ),
    }
}

impl HeadroomRun {
    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        let presentation = match tick {
            SessionTick::Preparing => return RunStep::Continue,
            SessionTick::Failed(failure) => return RunStep::Finish(Some(failure), None),
            SessionTick::BecameLive(presentation) => {
                if let Err(error) = self.start_bench(context, &presentation) {
                    return RunStep::Finish(None, Some(error));
                }
                presentation
            }
            SessionTick::Live(presentation) => presentation,
        };
        self.last = Some(presentation);
        self.record_minutes(context, running)
    }

    /// 120 Гц, стенд, запись минут; стадия `bench_started` — сигнал скрипту коснуться экрана.
    fn start_bench(
        &mut self,
        context: &mut ScenarioContext<'_>,
        presentation: &CameraPresentation,
    ) -> Result<(), String> {
        let hertz = self.config.headroom.display_refresh_hz;
        let outcome = context
            .gpu
            .request_display_refresh_for_scenario(context.services.commands.as_ref(), hertz)
            .map_err(|error| format!("частота экрана {hertz}: {error}"))?;
        self.refresh_request = Some(outcome);
        let delivery = context.gpu.set_headroom_bench_for_scenario(HeadroomBench::Enabled);
        if !delivery.is_delivered() {
            self.undelivered.push(format!("включение стенда: {delivery:?}"));
        }
        self.frames_at_start = presentation
            .viewfinder
            .as_ref()
            .map_or(0, |viewfinder| viewfinder.headroom.frames_recorded);
        self.minutes = Some(MinuteLog::start(
            context,
            &bench_power_settings(&self.power, &self.config),
        )?);
        context.emit_stage(
            "bench_started",
            stage_fields([
                ("display_refresh_hz", f64::from(hertz).into()),
                // Драйвер smoke касается экрана, только если просили больше 60 Гц.
                ("tap_for_refresh", (hertz > 60.0).into()),
                ("duration_ms", self.config.headroom.duration_ms.into()),
            ]),
        );
        Ok(())
    }

    fn record_minutes(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let Some(minutes) = &mut self.minutes else {
            return RunStep::Continue;
        };
        let last = self.last.as_deref();
        let bench_minutes = &mut self.bench_minutes;
        let mut closed = Vec::new();
        let step = minutes.tick(|| {
            let minute = u32::try_from(bench_minutes.len() + 1).unwrap_or(u32::MAX);
            let bench = bench_minute(minute, last);
            bench_minutes.push(bench);
            closed.push(bench);
            minute_observation(context, running, last)
        });
        for bench in closed {
            context.emit_stage(
                "minute",
                stage_fields([
                    ("minute", bench.minute.into()),
                    ("bench_total_p95_ms", bench.total.map(|total| total.p95_ms).into()),
                    ("refresh_hz", bench.refresh_hz.into()),
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
    ) -> HeadroomObservation {
        let settings = bench_power_settings(&self.power, &self.config);
        let (endurance, expected_minutes) = match &self.minutes {
            Some(minutes) => (minutes.recorder().report(), minutes.total_minutes()),
            None => {
                // Стенд не начался (отказ запуска): пустой отчёт с честным числом ожидаемых минут.
                let recorder = EnduranceRecorder::start(
                    &settings,
                    shell_contract::clock::MonotonicNanos::from_nanos(0),
                    ThermalSnapshot::read(context.services.thermal.as_ref()),
                );
                (recorder.report(), recorder.total_minutes())
            }
        };
        let last = self.last.as_deref();
        let bench = last
            .and_then(|presentation| presentation.viewfinder.as_ref())
            .map(|viewfinder| BenchSnapshot::of(&viewfinder.headroom, self.frames_at_start));
        let frame_budget_ms = self.config.headroom.frame_budget_ms();
        HeadroomObservation {
            failure,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            sensor_fps: running.handle().status().stream.measured_frames_per_second,
            frame_budget_ms,
            fits_frame_budget: fits_frame_budget(bench.as_ref(), frame_budget_ms),
            bench,
            display: DisplayDuringBench {
                requested_hz: self.config.headroom.display_refresh_hz,
                request: self.refresh_request,
                refresh_hz: DisplayDuringBench::refresh_hz_of(last.and_then(|last| last.display_refresh_cycle)),
                pacing_miss_ratio: last.map_or(0.0, |last| last.pacing.miss_ratio),
                pacing_intervals: last.map_or(0, |last| last.pacing.intervals),
            },
            expected_minutes,
            minutes: self.bench_minutes,
            endurance,
            undelivered: self.undelivered,
        }
    }
}

impl ObservingScenario for VideoHeadroomScenario {
    fn name(&self) -> &'static str {
        "video-headroom-bench"
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
        let step = match config.headroom_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&config)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        context.emit_stage(
            "started",
            stage_fields([
                ("duration_ms", config.headroom.duration_ms.into()),
                ("frame_rate", config.headroom.frame_rate.into()),
            ]),
        );
        self.run = Some(HeadroomRun {
            config,
            power,
            session,
            minutes: None,
            bench_minutes: Vec::new(),
            frames_at_start: 0,
            refresh_request: None,
            undelivered: Vec::new(),
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
        let RunStep::Finish(failure, run_error) = run.tick(context, running) else {
            return ScenarioStep::Continue;
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        if let Some(error) = run_error {
            return ScenarioStep::Finished(failed(error));
        }
        let observation = finished.into_observation(context, running, failure);
        emit_details(context, "detail_headroom", observation.detail_items());
        let violations = evaluate_headroom(&observation);
        ScenarioStep::Finished(finish_within_budget(context, observation.summary(), violations))
    }
}
