//! Сценарий `viewfinder-stage-budget` (сессия 07, решение 45): таблица «стадия → мс GPU → мА».
//!
//! Ход: общий запуск продуктового видоискателя, затем строки таблицы по порядку конфига
//! (`viewfinder.stage_budget.rows`). Каждая строка — свой A/B-замер тока
//! ([`AbPowerExperiment`], ABBA, `power.ab`): на каждом тике сценарий забирает показания батареи,
//! узнаёт, какое окно идёт и должна ли стадия быть включена, применяет это регулировкой видоискателя
//! (только при смене — команды идемпотентны, но засорять ящик рендера незачем) и в измеряемой части
//! окна запоминает снимок времени GPU. Отказ тока (внешнее питание, нет источника) прерывает
//! сценарий сразу: таблица без мА — не таблица решения 45, а шесть минут на строку жалко.
//!
//! Возврат рисовальщика и стадий к продукту делает маршрутизатор по концу сценария.

use isp_vulkan::viewfinder::IspStageSet;
use render_loop::viewfinder::ViewfinderPainter;
use shell_contract::power::{BatterySample, PowerSampling};

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::result_budget::finish_within_budget;
use super::viewfinder_metrics::GpuTimeMs;
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use super::viewfinder_stage_budget_report::{
    CameraWithoutIspRow, GpuWindowSample, RowPower, StageBudgetObservation, StageBudgetRow, evaluate_stage_budget,
};
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::clock_offset::monotonic_now;
use crate::power_measurement::{AbPowerExperiment, AbStep, Contribution, PowerSettings, StageSetting};
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_scenario_config::{StageBudgetRowSetting, ViewfinderScenarioConfig};

/// Настройка видоискателя для положения стадии в строке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewfinderSetting {
    pub painter: ViewfinderPainter,
    pub stages: IspStageSet,
}

/// Что включать в окнах A и B строки.
pub fn setting_for(row: StageBudgetRowSetting, stage: StageSetting) -> ViewfinderSetting {
    let product = ViewfinderSetting {
        painter: ViewfinderPainter::Isp,
        stages: IspStageSet::ALL,
    };
    match (row, stage) {
        (_, StageSetting::On) => product,
        (StageBudgetRowSetting::Stage(measured), StageSetting::Off) => ViewfinderSetting {
            painter: ViewfinderPainter::Isp,
            stages: IspStageSet::ALL.without(measured.to_stage()),
        },
        (StageBudgetRowSetting::WholeViewfinder, StageSetting::Off) => ViewfinderSetting {
            painter: ViewfinderPainter::DebugMosaic,
            stages: IspStageSet::ALL,
        },
    }
}

/// Действует ли настройка по статусу рендера.
fn in_effect(presentation: &CameraPresentation, setting: ViewfinderSetting) -> bool {
    presentation
        .viewfinder
        .as_ref()
        .is_some_and(|viewfinder| viewfinder.painter == setting.painter && viewfinder.stages == setting.stages)
}

/// Идущая строка.
struct OpenRow {
    row: StageBudgetRowSetting,
    experiment: AbPowerExperiment,
    applied: Option<StageSetting>,
    /// Последний снимок GPU в измеряемой части каждого окна (по номеру окна).
    windows: Vec<GpuWindowSample>,
}

struct BudgetRun {
    config: ViewfinderScenarioConfig,
    power: PowerSettings,
    session: ViewfinderSession,
    sampling: Option<PowerSampling>,
    open: Option<OpenRow>,
    next_row: usize,
    rows: Vec<StageBudgetRow>,
    power_refusal: Option<String>,
    undelivered: Vec<String>,
    samples: Vec<BatterySample>,
}

/// Сценарий `viewfinder-stage-budget`.
#[derive(Default)]
pub struct ViewfinderStageBudgetScenario {
    run: Option<BudgetRun>,
}

impl ViewfinderStageBudgetScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

enum RunStep {
    Continue,
    Finish(Option<RunFailure>),
}

impl BudgetRun {
    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        match tick {
            SessionTick::Preparing => RunStep::Continue,
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure)),
            SessionTick::BecameLive(_) => self.start_power(context),
            SessionTick::Live(presentation) => self.advance(context, &presentation),
        }
    }

    /// Запускает опрос батареи и первую строку; отказ опроса — честный конец сценария.
    fn start_power(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        match context.services.power.start_sampling(self.power.sampling) {
            Ok(sampling) => {
                self.sampling = Some(sampling);
                self.open_next_row(context)
            }
            Err(error) => {
                self.power_refusal = Some(format!("опрос батареи не запущен: {error}"));
                RunStep::Finish(None)
            }
        }
    }

    fn open_next_row(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        let Some(row) = self.config.stage_budget.rows.get(self.next_row).copied() else {
            return RunStep::Finish(None);
        };
        let started_at = match monotonic_now() {
            Ok(now) => now,
            Err(error) => {
                self.power_refusal = Some(format!("часы CLOCK_MONOTONIC не прочитаны: {error}"));
                return RunStep::Finish(None);
            }
        };
        self.next_row += 1;
        context.emit_stage(
            "row_started",
            stage_fields([("row", row.name().into()), ("index", (self.next_row - 1).into())]),
        );
        self.open = Some(OpenRow {
            row,
            experiment: AbPowerExperiment::new(&self.power, started_at),
            applied: None,
            windows: Vec::new(),
        });
        RunStep::Continue
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        let Some(sampling) = &self.sampling else {
            return RunStep::Continue;
        };
        self.samples.clear();
        sampling.drain_samples(&mut self.samples);
        let now = match monotonic_now() {
            Ok(now) => now,
            Err(error) => {
                self.power_refusal = Some(format!("часы CLOCK_MONOTONIC не прочитаны: {error}"));
                return RunStep::Finish(None);
            }
        };
        let Some(open) = &mut self.open else {
            return RunStep::Continue;
        };
        match open.experiment.on_tick(now, &self.samples) {
            AbStep::Measure { stage, window } => {
                if open.applied != Some(stage) {
                    open.applied = Some(stage);
                    let setting = setting_for(open.row, stage);
                    for delivery in [
                        context.gpu.set_viewfinder_painter_for_scenario(setting.painter),
                        context.gpu.set_isp_stages_for_scenario(setting.stages),
                    ] {
                        if !delivery.is_delivered() {
                            self.undelivered.push(format!(
                                "строка {}, окно {}: {delivery:?}",
                                open.row.name(),
                                window.index
                            ));
                        }
                    }
                }
                if !window.warming_up {
                    record_window(open, window.index, stage, presentation);
                }
                RunStep::Continue
            }
            AbStep::Finished => self.close_row(context),
        }
    }

    /// Строка кончилась: собрать её; отказ тока — прервать сценарий (дальше мерить бессмысленно).
    fn close_row(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        let Some(open) = self.open.take() else {
            return RunStep::Continue;
        };
        let power = match open.experiment.report() {
            Some(report) => RowPower::Ab { report: report.clone() },
            None => RowPower::SamplingUnavailable {
                reason: "A/B-замер кончился без отчёта".to_owned(),
            },
        };
        let rejected = match &power {
            RowPower::Ab { report } => match &report.contribution {
                Contribution::Rejected { reason } => Some(format!("{reason:?}")),
                _ => None,
            },
            RowPower::SamplingUnavailable { reason } => Some(reason.clone()),
        };
        let row = StageBudgetRow::assemble(open.row.name(), open.windows, power);
        context.emit_stage(
            "row_finished",
            stage_fields([("row", row.row.into()), ("line", row.table_line().into())]),
        );
        self.rows.push(row);
        if let Some(reason) = rejected {
            self.power_refusal = Some(format!("строка {}: {reason}", open.row.name()));
            return RunStep::Finish(None);
        }
        self.open_next_row(context)
    }

    fn into_observation(
        self,
        running: &RunningCamera,
        gpu_window_frames: usize,
        failure: Option<RunFailure>,
    ) -> StageBudgetObservation {
        StageBudgetObservation {
            failure,
            power_refusal: self.power_refusal,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            requested_rows: self.config.stage_budget.rows.iter().map(|row| row.name()).collect(),
            camera_without_isp: CameraWithoutIspRow::from_rows(&self.rows),
            rows: self.rows,
            gpu_window_frames,
            undelivered: self.undelivered,
        }
    }
}

/// Запоминает снимок времени GPU окна (последний в измеряемой части — самый полный).
fn record_window(open: &mut OpenRow, index: usize, stage: StageSetting, presentation: &CameraPresentation) {
    let viewfinder = presentation.viewfinder.as_ref();
    let sample = GpuWindowSample {
        index,
        stage,
        isp_dispatch: viewfinder
            .and_then(|viewfinder| GpuTimeMs::of_optional(viewfinder.gpu_time.isp_dispatch.as_ref())),
        frame_total: viewfinder.and_then(|viewfinder| GpuTimeMs::of_optional(viewfinder.gpu_time.frame_total.as_ref())),
        setting_in_effect: in_effect(presentation, setting_for(open.row, stage)),
    };
    match open.windows.iter_mut().find(|window| window.index == index) {
        Some(existing) => *existing = sample,
        None => open.windows.push(sample),
    }
}

impl ObservingScenario for ViewfinderStageBudgetScenario {
    fn name(&self) -> &'static str {
        "viewfinder-stage-budget"
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
        let rows: Vec<serde_json::Value> = config.stage_budget.rows.iter().map(|row| row.name().into()).collect();
        context.emit_stage(
            "started",
            stage_fields([
                ("rows", rows.into()),
                (
                    "window_ms",
                    u64::try_from(power.ab.window.as_millis()).unwrap_or(u64::MAX).into(),
                ),
                ("pairs", power.ab.pairs.into()),
            ]),
        );
        self.run = Some(BudgetRun {
            config,
            power,
            session,
            sampling: None,
            open: None,
            next_row: 0,
            rows: Vec::new(),
            power_refusal: None,
            undelivered: Vec::new(),
            samples: Vec::new(),
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
        let gpu_window_frames = context.gpu.render_config.viewfinder.gpu_time_window_frames;
        let observation = finished.into_observation(running, gpu_window_frames, failure);
        emit_details(context, "detail_stage_budget", observation.detail_items());
        let violations = evaluate_stage_budget(&observation);
        ScenarioStep::Finished(finish_within_budget(context, observation.summary(), violations))
    }
}
