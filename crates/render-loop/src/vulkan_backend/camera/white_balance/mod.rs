//! Наш AWB на потоке рендера (сессия 08, решения 62–68): владелец состояния «белая сцены» рядом со
//! статистикой GPU и параметрами ISP — без пересылок между потоками.
//!
//! # Кто чем владеет
//! * [`WhiteBalanceRunner`] — единственный, кто решает, какая белая идёт в ISP, показывать ли кадр
//!   (холодный старт) и нужна ли статистика на этом кадре;
//! * `scene_estimation::controller::WhiteBalanceController` — скалярное решение (режим, сглаживание);
//! * `scene_estimation::executor::WhiteBalanceEstimator` — тяжёлая часть (плитки → оценка); сегодня CPU-эталон,
//!   завтра compute-проход: runner знает только трейт (решение 68);
//! * [`SceneWhiteMemory`] — белая сцены между сессиями показа (владелец — бэкенд);
//! * `DirectiveSlot<WhiteBalanceDirective>` — режим AWB от `camera-core` (писатель — composition root).
//!
//! # Поток данных
//! `plan_frame` на каждый НОВЫЙ кадр камеры (до параметров ISP): применить свежее указание → шаг контроллера по
//! метке сенсора → решение холодного старта → белая для `IspParamsCache::prepare` и расписание статистики.
//! `ingest_statistics` — когда слот с кадром собран после fence: статистика уходит исполнителю, готовые оценки —
//! контроллеру. Оценка кадра всегда позже самого кадра минимум на `frames_in_flight` показов.
//!
//! # Инварианты
//! 1. В ISP идёт только белая нашего AWB (или временная D65 до первой оценки). Подсказки HAL — лишь диагностика.
//! 2. Статистика чужого потока (камера сменилась, пока слот был в полёте) оценщику не отдаётся: таблица
//!    локуса у каждой камеры своя.
//! 3. Скрытие кадров ограничено (`max_hidden_frames`) — картинка не прячется вечно.

mod cold_start;
mod hal_white;
mod memory;
mod schedule;
mod torch;

use std::sync::Arc;

use camera_contract::frame_passport::{RawFramePassport, StreamPassport};
use camera_contract::three_a::{DirectiveGeneration, DirectiveSlot, WhiteBalanceDirective};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::white_point::CctDuv;
use isp_model::statistics::AwbStatistics;
use scene_estimation::controller::{SceneWhite, WhiteBalanceController, WhiteStep};
use scene_estimation::estimate::{EstimateOutcome, IlluminantEstimate};
use scene_estimation::executor::{CpuWhiteBalanceEstimator, EstimationRequest, FrameTag, WhiteBalanceEstimator};
use scene_estimation::locus_table::CameraLocusTable;

use self::cold_start::{ColdStart, ColdStartVerdict};
use self::hal_white::{HalWhiteTracker, delta_between, reading_of};
pub use self::memory::SceneWhiteMemory;
pub use self::schedule::StatisticsPlan;
use self::schedule::StatisticsSchedule;
use self::torch::TorchWhiteTracker;
use super::flash_facts::TorchWhite;
use super::pipelines::IspPresentation;
use crate::config::WhiteBalanceConfig;
use crate::viewfinder::{AwbFrameIdentity, ColdStartState, EstimateReading, WhiteBalanceDiagnostics};

const LOG_TARGET: &str = "fasticamera::render::white_balance";

/// Белая, пока своей оценки нет: D65 — калибровочный свет профиля цвета (`ReferenceIlluminant::D65` в
/// ColorMatrix1 камеры), самый вероятный дневной свет. Кадры с ней либо скрыты (холодный старт), либо показаны
/// принудительно после лимита и помечены в диагностике.
pub const PROVISIONAL_WHITE: Chromaticity = Chromaticity::D65;

/// Поколение потока у AWB: растёт с каждым новым потоком камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwbStream(pub u64);

/// Что AWB даёт ISP и показу для одного нового кадра камеры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePlan {
    /// Поток, к которому относится кадр (метка для статистики слота).
    pub stream: AwbStream,
    /// Белая для параметров ISP этого кадра.
    pub white: Chromaticity,
    /// Показывать ли обработанный кадр.
    pub presentation: IspPresentation,
    /// Нужна ли статистика на этом кадре.
    pub statistics: StatisticsPlan,
}

/// Из чего собирается AWB сессии показа.
#[derive(Clone)]
pub struct WhiteBalanceParts {
    /// Настройки.
    pub config: WhiteBalanceConfig,
    /// Читающая половина «почтового ящика» указаний (писатель — у composition root → `camera-core`).
    pub directives: DirectiveSlot<WhiteBalanceDirective>,
    /// Память белой сцены бэкенда.
    pub memory: SceneWhiteMemory,
}

/// AWB не удалось собрать (конфиг отвергнут).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("настройки AWB: {0}")]
pub struct WhiteBalanceSetupError(pub String);

/// Зачем статистика кадра отдана оценке света.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatisticsPurpose {
    /// Обычный кадр видоискателя: оценка питает контроллер AWB.
    Viewfinder,
    /// Кадр с фонариком предвспышки: оценка света светодиода, мимо контроллера.
    Torch,
}

/// Привязка к потоку: профиль цвета и таблица локуса считаются один раз на поток.
struct BoundStream {
    passport: Arc<StreamPassport>,
    id: AwbStream,
    profile: Option<DngColorProfile>,
    table: Option<CameraLocusTable>,
}

/// Счётчики runner (то, что не хранит контроллер).
#[derive(Default)]
struct RunnerCounters {
    estimates: u64,
    estimation_failures: u64,
    estimator_busy: u64,
    statistics_requested: u64,
    statistics_skipped: u64,
    statistics_stale: u64,
    hidden_frames: u64,
    forced: u64,
    provisional_frames: u64,
}

/// AWB одной сессии показа (см. модуль).
pub struct WhiteBalanceRunner {
    config: WhiteBalanceConfig,
    directives: DirectiveSlot<WhiteBalanceDirective>,
    applied_generation: Option<DirectiveGeneration>,
    memory: SceneWhiteMemory,
    controller: WhiteBalanceController,
    estimator: Box<dyn WhiteBalanceEstimator + Send>,
    bound: Option<BoundStream>,
    next_stream_id: u64,
    cold_start: ColdStart,
    schedule: StatisticsSchedule,
    hal: HalWhiteTracker,
    hal_white: Option<CctDuv>,
    last_estimate: Option<IlluminantEstimate>,
    last_failure: Option<String>,
    table_failure: Option<String>,
    scene_white: Option<SceneWhite>,
    /// Белая светодиода по кадрам с фонариком предвспышки (14d): мимо контроллера видоискателя.
    torch: TorchWhiteTracker,
    counters: RunnerCounters,
}

impl WhiteBalanceRunner {
    /// Собирает AWB; белая сцены восстанавливается из памяти бэкенда (смена камеры / новая surface).
    pub fn new(parts: WhiteBalanceParts) -> Result<Self, WhiteBalanceSetupError> {
        let WhiteBalanceParts {
            config,
            directives,
            memory,
        } = parts;
        let setup = |error: &dyn std::fmt::Display| WhiteBalanceSetupError(error.to_string());
        let mut controller = WhiteBalanceController::new(config.scene).map_err(|error| setup(&error))?;
        let estimator = CpuWhiteBalanceEstimator::new(config.scene.estimator).map_err(|error| setup(&error))?;
        if let Some(remembered) = memory.load() {
            controller.restore_scene_white(remembered);
        }
        Ok(Self {
            config,
            directives,
            applied_generation: None,
            memory,
            controller,
            estimator: Box::new(estimator),
            bound: None,
            next_stream_id: 0,
            cold_start: ColdStart::new(config.max_hidden_frames),
            schedule: StatisticsSchedule::new(config.statistics_every_n_frames),
            hal: HalWhiteTracker::default(),
            hal_white: None,
            last_estimate: None,
            last_failure: None,
            table_failure: None,
            scene_white: None,
            torch: TorchWhiteTracker::default(),
            counters: RunnerCounters::default(),
        })
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток. Белая сцены остаётся.
    pub fn forget_stream(&mut self) {
        self.bound = None;
    }

    /// План для нового кадра камеры (см. модуль). Вызывается до подготовки параметров ISP.
    pub fn plan_frame(&mut self, passport: &RawFramePassport) -> FramePlan {
        self.refresh_directive();
        let stream = self.bind_stream(passport);
        let sensor_timestamp = passport.sensor_timestamp.as_nanos();
        let mut step = self.step(sensor_timestamp);
        let cold_start_before = self.cold_start.state();
        let verdict = self.cold_start.decide(self.controller.has_scene_white());
        if let ColdStartVerdict::ShowForced { .. } = verdict
            && let Some(estimate) = self.last_estimate
            && self.controller.adopt_best_effort(&estimate)
        {
            // Лимит исчерпан: лучшая доступная белая — слабая оценка; без неё останется временная D65.
            // Тот же момент времени: шаг контроллера нулевой, меняется только стартовая точка.
            step = self.step(sensor_timestamp);
        }
        self.note_cold_start(cold_start_before, verdict);
        self.note_hal(passport);
        self.note_step(&step);
        let statistics = self.statistics_plan(verdict, stream);
        FramePlan {
            stream,
            white: step.white.map_or(PROVISIONAL_WHITE, |white| white.chromaticity),
            presentation: match verdict {
                ColdStartVerdict::Hide => IspPresentation::HideUntilColorKnown,
                ColdStartVerdict::Show | ColdStartVerdict::ShowForced { .. } => IspPresentation::Show,
            },
            statistics,
        }
    }

    /// Статистика кадра слота (собрана после fence): исполнителю, готовые оценки — контроллеру.
    pub fn ingest_statistics(&mut self, stream: AwbStream, frame: AwbFrameIdentity, statistics: &AwbStatistics) {
        self.submit_statistics(stream, frame, statistics, StatisticsPurpose::Viewfinder);
    }

    /// Статистика кадра с фонариком предвспышки: оценка света светодиода, НЕ влияющая на белую видоискателя.
    /// Отдаётся независимо от расписания AWB: предвспышка — единицы кадров, и пропустить кадр с фонариком по
    /// прореживанию значило бы остаться без белой светодиода.
    pub fn ingest_torch_statistics(&mut self, stream: AwbStream, frame: AwbFrameIdentity, statistics: &AwbStatistics) {
        self.submit_statistics(stream, frame, statistics, StatisticsPurpose::Torch);
    }

    /// Новые белые светодиода для движка снимков (список становится пустым).
    pub fn drain_torch_whites(&mut self) -> Vec<TorchWhite> {
        self.torch.drain()
    }

    fn submit_statistics(
        &mut self,
        stream: AwbStream,
        frame: AwbFrameIdentity,
        statistics: &AwbStatistics,
        purpose: StatisticsPurpose,
    ) {
        let Some(bound) = self.bound.as_ref().filter(|bound| bound.id == stream) else {
            self.counters.statistics_stale += 1;
            return;
        };
        let Some(table) = bound.table.as_ref() else {
            return;
        };
        let request = EstimationRequest {
            frame: FrameTag {
                stream_generation: stream.0,
                sequence: frame.sequence,
                sensor_timestamp_nanos: frame.sensor_timestamp_nanos,
            },
            statistics,
            table,
        };
        if purpose == StatisticsPurpose::Torch {
            self.torch.note_submitted(frame.sensor_timestamp_nanos);
        }
        if self.estimator.submit(request).is_err() {
            self.counters.estimator_busy += 1;
            if purpose == StatisticsPurpose::Torch {
                self.torch.withdraw(frame.sensor_timestamp_nanos);
            }
        }
        self.collect_estimates(stream);
    }

    /// Снимок диагностики.
    pub fn diagnostics(&self) -> WhiteBalanceDiagnostics {
        let ours = self.scene_white.map(|white| white.cct_duv);
        WhiteBalanceDiagnostics {
            directive: self.controller.directive(),
            directive_generation: self.applied_generation.map(|generation| generation.0),
            scene_white: ours.map(reading_of),
            last_estimate: self.last_estimate.map(|estimate| EstimateReading {
                white: reading_of(estimate.cct_duv),
                confidence: estimate.confidence,
                accepted_tiles: estimate.accepted_tiles,
                clamped_to_band: estimate.clamped.is_any(),
            }),
            manual_limit: self.controller.manual_limit(),
            hal_white: self.hal_white.map(reading_of),
            hal_delta: ours.zip(self.hal_white).map(|(ours, hal)| delta_between(ours, hal)),
            estimates: self.counters.estimates,
            decisions: self.controller.counters(),
            estimation_failures: self.counters.estimation_failures,
            estimator_busy: self.counters.estimator_busy,
            statistics_every_n_frames: self.config.statistics_every_n_frames,
            statistics_requested: self.counters.statistics_requested,
            statistics_skipped_by_decimation: self.counters.statistics_skipped,
            statistics_stale: self.counters.statistics_stale,
            cold_start: self.cold_start.state(),
            hidden_frames_cold_start: self.counters.hidden_frames,
            cold_start_forced: self.counters.forced,
            frames_with_provisional_white: self.counters.provisional_frames,
            table_failure: self.table_failure.clone(),
            torch_white_estimates: self.torch.estimated,
            torch_white_failures: self.torch.failed,
        }
    }

    // ---- внутреннее ----

    /// Свежее указание от владельца 3A (если появилось): применяется один раз на поколение.
    fn refresh_directive(&mut self) {
        let Some((generation, directive)) = self.directives.newer_than(self.applied_generation) else {
            return;
        };
        self.controller.set_directive(directive);
        self.applied_generation = Some(generation);
        // Раз на смену указания, не на кадр.
        tracing::info!(target: LOG_TARGET, generation = generation.0, ?directive, "указание AWB применено");
    }

    /// Привязывает к потоку кадра; новый поток строит профиль и таблицу локуса и начинает холодный старт.
    fn bind_stream(&mut self, passport: &RawFramePassport) -> AwbStream {
        if let Some(bound) = &self.bound
            && Arc::ptr_eq(&bound.passport, &passport.stream)
        {
            return bound.id;
        }
        self.next_stream_id += 1;
        let id = AwbStream(self.next_stream_id);
        let stream = &passport.stream;
        let profile = DngColorProfile::from_calibration_sets(&stream.color_calibration);
        let scene = &self.config.scene;
        let (profile, table, failure) = match profile {
            Ok(profile) => match CameraLocusTable::build(&profile, &scene.locus, scene.estimator.result_max_abs_duv) {
                Ok(table) => (Some(profile), Some(table), None),
                Err(error) => (Some(profile), None, Some(format!("таблица локуса: {error}"))),
            },
            Err(error) => (None, None, Some(format!("профиль цвета потока: {error}"))),
        };
        if let Some(failure) = &failure {
            tracing::warn!(target: LOG_TARGET, %failure, "AWB потока не работает: белая будет временной");
        }
        self.table_failure = failure;
        // Оценки старого потока исполнитель мог ещё не отдать: они чужие.
        while self.estimator.poll().is_some() {}
        self.controller.begin_stream();
        self.hal.reset();
        self.hal_white = None;
        self.last_estimate = None;
        self.torch.reset();
        let scene_white_known = self.controller.has_scene_white();
        // Без таблицы оценок не будет никогда: ждать нечего, прятать кадры незачем.
        self.cold_start.begin_stream(scene_white_known || table.is_none());
        self.schedule.restart();
        tracing::info!(
            target: LOG_TARGET,
            stream = id.0,
            scene_white_known,
            "новый поток AWB"
        );
        self.bound = Some(BoundStream {
            passport: Arc::clone(stream),
            id,
            profile,
            table,
        });
        id
    }

    /// Шаг контроллера по метке сенсора кадра.
    fn step(&mut self, sensor_timestamp_nanos: i64) -> WhiteStep {
        match self.controller.advance(sensor_timestamp_nanos) {
            Ok(step) => step,
            Err(error) => {
                self.counters.estimation_failures += 1;
                self.note_failure(format!("белая сцены не переводится в цветность: {error}"));
                WhiteStep {
                    white: None,
                    changed: false,
                }
            }
        }
    }

    /// Запоминает результат шага: диагностика, память белой между сессиями.
    fn note_step(&mut self, step: &WhiteStep) {
        if step.white.is_none() {
            self.counters.provisional_frames += 1;
        }
        self.scene_white = step.white.or(self.scene_white);
        if step.changed
            && let Some(state) = self.controller.current_state()
        {
            self.memory.store(state);
        }
    }

    /// Счётчики и журнал холодного старта (журнал — только на границах состояния, не на кадр).
    fn note_cold_start(&mut self, before: ColdStartState, verdict: ColdStartVerdict) {
        match verdict {
            ColdStartVerdict::Hide => self.counters.hidden_frames += 1,
            ColdStartVerdict::ShowForced { hidden } => {
                self.counters.forced += 1;
                tracing::warn!(
                    target: LOG_TARGET,
                    hidden,
                    "холодный старт AWB: оценки нет, кадры показаны с лучшей доступной белой"
                );
            }
            ColdStartVerdict::Show => {
                if let ColdStartState::Pending { hidden } = before {
                    tracing::info!(target: LOG_TARGET, hidden, "холодный старт AWB закончен первой оценкой");
                }
            }
        }
    }

    /// Белая HAL для сравнения (только диагностика).
    fn note_hal(&mut self, passport: &RawFramePassport) {
        let Some(details) = passport.capture.details() else {
            return;
        };
        let Some(profile) = self.bound.as_ref().and_then(|bound| bound.profile.as_ref()) else {
            return;
        };
        self.hal_white = self.hal.observe(&details.color, profile);
    }

    /// Расписание статистики с учётом холодного старта и наличия таблицы.
    fn statistics_plan(&mut self, verdict: ColdStartVerdict, stream: AwbStream) -> StatisticsPlan {
        let has_table = self
            .bound
            .as_ref()
            .is_some_and(|bound| bound.id == stream && bound.table.is_some());
        if !has_table {
            return StatisticsPlan::Skip;
        }
        let waiting = verdict == ColdStartVerdict::Hide;
        let plan = self.schedule.decide(waiting);
        match plan {
            StatisticsPlan::Compute => self.counters.statistics_requested += 1,
            StatisticsPlan::Skip => self.counters.statistics_skipped += 1,
        }
        plan
    }

    /// Забирает готовые оценки у исполнителя и отдаёт контроллеру.
    fn collect_estimates(&mut self, current: AwbStream) {
        while let Some(done) = self.estimator.poll() {
            if done.frame.stream_generation != current.0 {
                self.counters.statistics_stale += 1;
                continue;
            }
            self.counters.estimates += 1;
            if self.torch.claim(done.frame.sensor_timestamp_nanos) {
                // Кадр с фонариком: оценка света светодиода, контроллер видоискателя её не видит.
                match (&done.outcome, self.bound.as_ref()) {
                    (Ok(EstimateOutcome::Estimated(estimate)), Some(bound)) => {
                        self.torch
                            .record(&bound.passport, done.frame.sensor_timestamp_nanos, estimate.white);
                    }
                    _ => self.torch.record_failure(),
                }
                continue;
            }
            match done.outcome {
                Ok(outcome) => {
                    if let EstimateOutcome::Estimated(estimate) = &outcome {
                        self.last_estimate = Some(*estimate);
                    }
                    self.controller.ingest(&outcome);
                }
                Err(error) => {
                    self.counters.estimation_failures += 1;
                    self.note_failure(format!("оценка света: {error}"));
                }
            }
        }
    }

    /// Ошибку в журнал — один раз на новый текст, не на кадр.
    fn note_failure(&mut self, message: String) {
        if self.last_failure.as_deref() != Some(message.as_str()) {
            tracing::warn!(target: LOG_TARGET, %message, "AWB: сбой");
            self.last_failure = Some(message);
        }
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;
#[cfg(test)]
#[path = "torch_tests.rs"]
mod torch_tests;
