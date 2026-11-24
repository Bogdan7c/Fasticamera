//! Белая на потоке рендера (white-balance 01; прежний AWB сессии 08 в продукт больше не идёт).
//!
//! # Кто чем владеет
//! * [`WhiteBalanceRunner`] — единственный, кто решает, какая белая идёт в ISP, показывать ли кадр
//!   (холодный старт) и нужна ли статистика на этом кадре;
//! * `applied::AppliedWhite` — применяемая белая: цель владельца, переход по меткам кадров, побитовая цель после него;
//! * `sources::SourceExecutor` — источники белого из реестра: заявка владельца → ответ в ящик движку;
//! * `cold_start::ColdStart` — скрытие кадров, пока действует заявка `HideFrames`;
//! * `scene_estimation::executor::WhiteBalanceEstimator` — старый оценщик (плитки → оценка). Его оценки в
//!   картинку НЕ идут (`last_estimate` — диагностика), но живы ради предвспышки (`TorchWhiteTracker`) и
//!   расписания статистики, на котором стоит AE (удалит сессия 03 плана);
//! * `DirectiveSlot<WhiteDirective>` — указания владельца белого (`camera-core`).
//!
//! # Поток данных
//! `plan_frame` на каждый НОВЫЙ показываемый кадр камеры (до параметров ISP): свежее указание → привязка к
//! потоку (источникам `begin_stream`) → сверка заявки и кадр источникам → решение холодного старта → шаг
//! применяемой белой → расписание статистики. `ingest_statistics` — когда слот с кадром собран после fence:
//! статистика уходит старому оценщику (для диагностики и предвспышки).
//!
//! # Инварианты
//! 1. В ISP идёт только белая из указания владельца (или временная D65, пока указаний не было вовсе). Подсказки
//!    HAL, оценки старого оценщика — лишь диагностика. Без нового указания белая не меняется ни на бит.
//! 2. Статистика чужого потока оценщику не отдаётся: таблица локуса у каждой камеры своя.
//! 3. Скрытие кадров ограничено (`max_hidden_frames`) — картинка не прячется вечно.

mod applied;
mod cold_start;
mod diagnostics;
mod hal_white;
mod schedule;
mod sources;
mod torch;

use std::sync::Arc;

use camera_contract::auto_exposure_carry::CameraTag;
use camera_contract::frame_passport::{RawFramePassport, StreamPassport};
use camera_contract::three_a::DirectiveGeneration;
use camera_contract::white::{WhiteDirective, WhiteSourceRequest};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::white_point::CctDuv;
use isp_model::statistics::AwbStatistics;
use scene_estimation::estimate::{EstimateOutcome, IlluminantEstimate};
use scene_estimation::executor::{CpuWhiteBalanceEstimator, EstimationRequest, FrameTag, WhiteBalanceEstimator};
use scene_estimation::locus_table::CameraLocusTable;

use self::applied::AppliedWhite;
use self::cold_start::{ColdStart, ColdStartVerdict};
use self::hal_white::HalWhiteTracker;
pub use self::schedule::StatisticsPlan;
use self::schedule::StatisticsSchedule;
use self::sources::SourceExecutor;
use self::torch::TorchWhiteTracker;
use super::flash_facts::TorchWhite;
use super::pipelines::IspPresentation;
use crate::config::WhiteBalanceConfig;
use crate::viewfinder::{AwbFrameIdentity, ColdStartState, WhiteLinks};

const LOG_TARGET: &str = "fasticamera::render::white_balance";

/// Поколение потока у AWB: растёт с каждым новым потоком камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwbStream(pub u64);

/// Что белая даёт ISP и показу для одного нового кадра камеры.
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

/// Из чего собирается белая сессии показа.
#[derive(Clone)]
pub struct WhiteBalanceParts {
    /// Настройки.
    pub config: WhiteBalanceConfig,
    /// Связка с владельцем белого: указания, ящик ответов, источники.
    pub links: WhiteLinks,
}

/// Белую не удалось собрать (конфиг отвергнут или фабрика источника отказала).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("настройки AWB: {0}")]
pub struct WhiteBalanceSetupError(pub String);

/// Зачем статистика кадра отдана оценке света.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatisticsPurpose {
    /// Обычный кадр видоискателя: оценка только для диагностики.
    Viewfinder,
    /// Кадр с фонариком предвспышки: оценка света светодиода.
    Torch,
}

/// Привязка к потоку: профиль цвета и таблица локуса считаются один раз на поток.
struct BoundStream {
    passport: Arc<StreamPassport>,
    id: AwbStream,
    profile: Option<DngColorProfile>,
    table: Option<CameraLocusTable>,
}

/// Счётчики runner.
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

/// Белая одной сессии показа (см. модуль).
pub struct WhiteBalanceRunner {
    config: WhiteBalanceConfig,
    directives: camera_contract::three_a::DirectiveSlot<WhiteDirective>,
    applied_generation: Option<DirectiveGeneration>,
    /// Заявка источнику действующего указания.
    request: Option<WhiteSourceRequest>,
    applied: AppliedWhite,
    sources: SourceExecutor,
    cold_start: ColdStart,
    estimator: Box<dyn WhiteBalanceEstimator + Send>,
    bound: Option<BoundStream>,
    next_stream_id: u64,
    schedule: StatisticsSchedule,
    hal: HalWhiteTracker,
    hal_white: Option<CctDuv>,
    last_estimate: Option<IlluminantEstimate>,
    last_failure: Option<String>,
    table_failure: Option<String>,
    /// Белая светодиода по кадрам с фонариком предвспышки (14d).
    torch: TorchWhiteTracker,
    counters: RunnerCounters,
}

impl WhiteBalanceRunner {
    /// Собирает белую сессии показа: свежие источники из реестра, старый оценщик для диагностики и предвспышки.
    pub fn new(parts: WhiteBalanceParts) -> Result<Self, WhiteBalanceSetupError> {
        let WhiteBalanceParts { config, links } = parts;
        let setup = |error: &dyn std::fmt::Display| WhiteBalanceSetupError(error.to_string());
        let estimator = CpuWhiteBalanceEstimator::new(config.scene.estimator).map_err(|error| setup(&error))?;
        let sources = SourceExecutor::new(&links.sources, links.answers.clone())?;
        Ok(Self {
            config,
            directives: links.directives,
            applied_generation: None,
            request: None,
            applied: AppliedWhite::default(),
            sources,
            cold_start: ColdStart::new(config.max_hidden_frames),
            estimator: Box::new(estimator),
            bound: None,
            next_stream_id: 0,
            schedule: StatisticsSchedule::new(config.statistics_every_n_frames),
            hal: HalWhiteTracker::default(),
            hal_white: None,
            last_estimate: None,
            last_failure: None,
            table_failure: None,
            torch: TorchWhiteTracker::default(),
            counters: RunnerCounters::default(),
        })
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток. Применённая белая остаётся.
    pub fn forget_stream(&mut self) {
        self.bound = None;
    }

    /// План для нового кадра камеры (см. модуль). Вызывается до подготовки параметров ISP.
    pub fn plan_frame(&mut self, passport: &RawFramePassport) -> FramePlan {
        self.refresh_directive();
        let stream = self.bind_stream(passport);
        self.sources.sync_request(self.request);
        self.sources.observe(passport);
        self.sources.collect_answers();
        let cold_start_before = self.cold_start.state();
        let verdict = self
            .cold_start
            .decide(self.request.map(|request| request.while_awaiting));
        let shown = verdict != ColdStartVerdict::Hide;
        let applied = self.applied.advance(passport.sensor_timestamp.as_nanos(), shown);
        self.note_cold_start(cold_start_before, verdict);
        self.note_hal(passport);
        if applied.provisional {
            self.counters.provisional_frames += 1;
        }
        let statistics = self.statistics_plan(stream, !shown);
        FramePlan {
            stream,
            white: applied.white,
            presentation: if shown {
                IspPresentation::Show
            } else {
                IspPresentation::HideUntilColorKnown
            },
            statistics,
        }
    }

    /// Статистика кадра слота (собрана после fence): старому оценщику, готовые оценки — в диагностику.
    pub fn ingest_statistics(&mut self, stream: AwbStream, frame: AwbFrameIdentity, statistics: &AwbStatistics) {
        self.submit_statistics(stream, frame, statistics, StatisticsPurpose::Viewfinder);
    }

    /// Статистика кадра с фонариком предвспышки: оценка света светодиода, НЕ влияющая на белую видоискателя.
    /// Отдаётся независимо от расписания: предвспышка — единицы кадров, и пропустить кадр с фонариком по
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

    // ---- внутреннее ----

    /// Свежее указание владельца (если появилось): применяется один раз на поколение. Первое указание сессии
    /// показа — «последнее из ящика», то есть и указание, опубликованное до создания сессии.
    fn refresh_directive(&mut self) {
        let Some((generation, directive)) = self.directives.newer_than(self.applied_generation) else {
            return;
        };
        self.applied.accept(&directive);
        self.request = directive.request;
        self.applied_generation = Some(generation);
        // Раз на смену указания, не на кадр.
        tracing::info!(target: LOG_TARGET, generation = generation.0, ?directive, "указание белой применено");
    }

    /// Привязывает к потоку кадра; новый поток строит профиль и таблицу локуса и сообщает источникам.
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
            tracing::warn!(target: LOG_TARGET, %failure, "оценщик потока не работает (белая в картинке не зависит)");
        }
        self.table_failure = failure;
        // Оценки старого потока исполнитель мог ещё не отдать: они чужие.
        while self.estimator.poll().is_some() {}
        self.sources
            .begin_stream(CameraTag::of(&stream.camera), profile.as_ref());
        self.hal.reset();
        self.hal_white = None;
        self.last_estimate = None;
        self.torch.reset();
        self.schedule.restart();
        tracing::info!(target: LOG_TARGET, stream = id.0, "новый поток белой");
        self.bound = Some(BoundStream {
            passport: Arc::clone(stream),
            id,
            profile,
            table,
        });
        id
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
                    "холодный старт: ответа источника нет, кадры показаны с текущей белой"
                );
            }
            ColdStartVerdict::Show => {
                if let ColdStartState::Pending { hidden } = before {
                    tracing::info!(target: LOG_TARGET, hidden, "холодный старт закончен закрытием заявки");
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

    /// Расписание статистики: пока кадры спрятаны холодным стартом — на каждом кадре, потом прореживание
    /// (как было при ожидании первой оценки: на статистике стоит AE).
    fn statistics_plan(&mut self, stream: AwbStream, hidden_by_cold_start: bool) -> StatisticsPlan {
        let has_table = self
            .bound
            .as_ref()
            .is_some_and(|bound| bound.id == stream && bound.table.is_some());
        if !has_table {
            return StatisticsPlan::Skip;
        }
        let plan = self.schedule.decide(hidden_by_cold_start);
        match plan {
            StatisticsPlan::Compute => self.counters.statistics_requested += 1,
            StatisticsPlan::Skip => self.counters.statistics_skipped += 1,
        }
        plan
    }

    /// Забирает готовые оценки у исполнителя: кадры с фонариком — вспышке, остальные — в диагностику.
    fn collect_estimates(&mut self, current: AwbStream) {
        while let Some(done) = self.estimator.poll() {
            if done.frame.stream_generation != current.0 {
                self.counters.statistics_stale += 1;
                continue;
            }
            self.counters.estimates += 1;
            if self.torch.claim(done.frame.sensor_timestamp_nanos) {
                // Кадр с фонариком: оценка света светодиода.
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
            tracing::warn!(target: LOG_TARGET, %message, "белая: сбой");
            self.last_failure = Some(message);
        }
    }
}

#[cfg(test)]
#[path = "applied_tests.rs"]
mod applied_tests;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod runner_tests;
#[cfg(test)]
#[path = "sources_tests.rs"]
mod sources_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "torch_tests.rs"]
mod torch_tests;
