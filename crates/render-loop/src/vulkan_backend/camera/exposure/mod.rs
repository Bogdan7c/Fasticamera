//! Наш AE на потоке рендера (сессия 08a, решения 63, 66, 68): «разведчик» — меряет сцену по той же
//! статистике, что и AWB, и предлагает движку камеры, сколько света нужно.
//!
//! # Кто чем владеет
//! * [`ExposureRunner`] — единственный, кто на потоке рендера решает: какая остаточная подтяжка идёт в ISP,
//!   нужна ли статистика на этом кадре (холодный старт замера), какое предложение уходит движку;
//! * `scene_estimation::exposure::AutoExposureController` — скалярное решение (цель, сглаживание, блокировка);
//! * `scene_estimation::exposure::ExposureMeter` — тяжёлая часть (плитки → требуемое произведение); сегодня CPU,
//!   завтра compute-проход: runner знает только трейт;
//! * `DirectiveSlot<ExposureDirective>` — указание движка (где мерить, кто ведёт экспозицию, что стоит в запросе);
//! * `ExposureProposalChannel` — ящик предложений движку (с «толчком»).
//!
//! # Поток данных
//! `plan_frame` на каждый НОВЫЙ показываемый кадр (до параметров ISP): свежее указание → привязка к потоку →
//! шаг цели по метке сенсора → предложение в ящик → подтяжка `k` для `IspParamsCache::prepare` и запрос
//! статистики. `ingest_statistics` — когда слот с кадром собран после fence: замер, его приём контроллером.
//!
//! # Инварианты
//! 1. Выдержку и ISO кадра НЕ подставляем: замер использует только значения результата, спаренного с кадром
//!    (`FrameCaptureMeta::Paired`); нет пары или ключа — кадр пропущен со счётчиком, подтяжка единичная.
//! 2. Предложение старого потока новому не достаётся: смена потока сбрасывает контроллер (цели нет, пока нет
//!    замера нового потока), а статистика чужого потока отбрасывается.
//! 3. Кадры серии (`RequestOrigin::Burst`) сюда не доходят: их отпускают до `plan_frame`.
//! 4. Ручная экспозиция: замер ничего не решает, статистику «на каждом кадре» AE не просит.

mod placement;

use std::sync::Arc;

use camera_contract::auto_exposure::{
    ExposureAuthority, ExposureDirective, ExposureProduct, ExposureProposal, ExposureProposalChannel, MeteredArea,
};
use camera_contract::frame_passport::{FrameCaptureMeta, RawFramePassport, StreamPassport};
use camera_contract::geometry::PixelRect;
use camera_contract::three_a::{DirectiveGeneration, DirectiveSlot, PointOfInterest};
use isp_model::exposure::{ExposureGain, LuminanceWeights};
use isp_model::params::IspParams;
use isp_model::statistics::AwbStatistics;
use scene_estimation::error::ConfigError;
use scene_estimation::executor::FrameTag;
use scene_estimation::exposure::{
    AutoExposureController, CpuExposureMeter, ExposureMeter, ExposureMeteringRequest, ExposureTarget, IngestOutcome,
    MeteringArea, MeteringWeights, MeteringWeightsError, SkippedMeasurement,
};

use self::placement::tile_placement;
use super::statistics_schedule::StatisticsDemand;
use crate::config::AutoExposureConfig;
use crate::viewfinder::{AutoExposureCounters, AutoExposureDiagnostics, AwbFrameIdentity};

const LOG_TARGET: &str = "fasticamera::render::auto_exposure";

/// Поколение потока у AE: растёт с каждым новым потоком камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureStream(pub u64);

/// Из чего собирается AE сессии показа.
#[derive(Clone)]
pub struct AutoExposureParts {
    /// Настройки (замер, контроллер, холодный старт).
    pub config: AutoExposureConfig,
    /// Читающая половина ящика указаний (писатель — движок камеры).
    pub directives: DirectiveSlot<ExposureDirective>,
    /// Ящик предложений движку (писатель — этот runner).
    pub proposals: ExposureProposalChannel,
}

/// AE не удалось собрать (конфиг отвергнут).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("настройки AE: {0}")]
pub struct AutoExposureSetupError(pub String);

/// Что AE даёт ISP и расписанию статистики для одного нового кадра камеры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureFramePlan {
    /// Поток, к которому относится кадр (метка для статистики слота).
    pub stream: ExposureStream,
    /// Остаточная подтяжка этого кадра для `IspParamsCache::prepare` (`UNITY` — нет данных или не нужна).
    pub gain: ExposureGain,
    /// Чего AE хочет от статистики на этом кадре.
    pub statistics: StatisticsDemand,
}

/// Факты кадра, нужные замеру. Известны только после `prepare` параметров ISP (веса яркости) и геометрии.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureFrameFacts {
    /// «Выдержка × ISO» кадра из его результата; `None` — спаренного результата с этими ключами нет.
    pub product: Option<ExposureProduct>,
    /// Веса яркости из `IspParams`, с которыми обработан ЭТОТ кадр.
    pub luminance: LuminanceWeights,
    /// Кадрирование, по которому ISP посчитал плитки статистики (координаты буфера).
    pub crop: PixelRect,
}

impl ExposureFrameFacts {
    /// Факты кадра `passport`, обработанного параметрами `params` с кадрированием `crop`.
    pub fn of_frame(passport: &RawFramePassport, params: &IspParams, crop: PixelRect) -> Self {
        Self {
            product: frame_exposure_product(passport),
            luminance: params.scene_luminance_weights(),
            crop,
        }
    }
}

/// Всё, что слот кольца помнит о кадре для AE: поток и факты замера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureSlotFrame {
    /// Поток кадра.
    pub stream: ExposureStream,
    /// Факты замера.
    pub facts: ExposureFrameFacts,
}

/// «Выдержка × ISO» кадра из спаренного результата. `None`, если пары нет или в ней нет выдержки / ISO / они
/// нулевые: подставлять значения соседа или запроса нельзя (инвариант 1).
pub fn frame_exposure_product(passport: &RawFramePassport) -> Option<ExposureProduct> {
    let FrameCaptureMeta::Paired(details) = &passport.capture else {
        return None;
    };
    ExposureProduct::from_result(details.result.exposure_time?, details.result.sensitivity_iso?)
}

/// Привязка к потоку.
struct BoundStream {
    passport: Arc<StreamPassport>,
    id: ExposureStream,
}

/// Веса плиток под конкретные поток, кадрирование и область, либо причина, почему их построить нельзя.
struct CachedWeights {
    key: WeightsKey,
    weights: Result<MeteringWeights, WeightsUnavailable>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WeightsKey {
    stream: ExposureStream,
    crop: PixelRect,
    area: MeteredArea,
}

/// Почему область замера построить не удалось.
#[derive(Debug, Clone, PartialEq)]
enum WeightsUnavailable {
    /// Регион тапа не пересекает ни одной плитки сетки (например, вне вырезки 16:9).
    RegionOutsideGrid,
    /// Положение сетки или веса не посчитались (причина в тексте).
    Failed(MeteringWeightsError),
}

/// Счётчики runner (то, что не хранит контроллер).
#[derive(Default)]
struct RunnerCounters {
    skipped_area_unavailable: u64,
    statistics_stale: u64,
    cold_start_statistics_frames: u64,
    frames_with_residual_gain: u64,
}

/// AE одной сессии показа (см. модуль).
pub struct ExposureRunner {
    config: AutoExposureConfig,
    directives: DirectiveSlot<ExposureDirective>,
    proposals: ExposureProposalChannel,
    applied_generation: Option<DirectiveGeneration>,
    directive: ExposureDirective,
    controller: AutoExposureController,
    meter: Box<dyn ExposureMeter + Send>,
    target: ExposureTarget,
    bound: Option<BoundStream>,
    next_stream_id: u64,
    weights: Option<CachedWeights>,
    /// Принят ли замер для текущей области текущего потока (пока нет — холодный старт).
    has_estimate: bool,
    /// Сколько кадров подряд AE уже просил статистику «на каждом кадре» в этом холодном старте.
    cold_start_frames: u32,
    /// Регион текущей области лежит вне сетки: ждать оценки бессмысленно.
    area_unavailable: bool,
    last_gain: ExposureGain,
    last_proposal: Option<ExposureProposal>,
    counters: RunnerCounters,
}

impl ExposureRunner {
    /// Собирает AE. `scene_middle_gray` — цель яркости из настроек тона ISP, которые реально использует
    /// рендер (число не дублируется: его передаёт вызывающий).
    pub fn new(parts: AutoExposureParts, scene_middle_gray: f32) -> Result<Self, AutoExposureSetupError> {
        let AutoExposureParts {
            config,
            directives,
            proposals,
        } = parts;
        let setup = |error: &ConfigError| AutoExposureSetupError(error.to_string());
        let controller = AutoExposureController::new(config.controller).map_err(|error| setup(&error))?;
        let meter = CpuExposureMeter::new(config.metering).map_err(|error| setup(&error))?;
        let target = ExposureTarget::new(scene_middle_gray).map_err(|error| setup(&error))?;
        Ok(Self {
            config,
            directives,
            proposals,
            applied_generation: None,
            directive: ExposureDirective::DEFAULT,
            controller,
            meter: Box::new(meter),
            target,
            bound: None,
            next_stream_id: 0,
            weights: None,
            has_estimate: false,
            cold_start_frames: 0,
            area_unavailable: false,
            last_gain: ExposureGain::UNITY,
            last_proposal: None,
            counters: RunnerCounters::default(),
        })
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток.
    pub fn forget_stream(&mut self) {
        self.bound = None;
    }

    /// План для нового кадра камеры (см. модуль). Вызывается до подготовки параметров ISP.
    pub fn plan_frame(&mut self, passport: &RawFramePassport) -> ExposureFramePlan {
        self.refresh_directive();
        let stream = self.bind_stream(passport);
        // Шаг цели и предложение — по метке сенсора показываемого кадра.
        self.controller.advance(passport.sensor_timestamp);
        if let Some(proposal) = self.controller.proposal(passport.sensor_timestamp) {
            self.proposals.publish(proposal);
            self.last_proposal = Some(proposal);
        }
        let gain = self.quantized(self.controller.display_gain(frame_exposure_product(passport)));
        if gain != ExposureGain::UNITY {
            self.counters.frames_with_residual_gain += 1;
        }
        self.last_gain = gain;
        ExposureFramePlan {
            stream,
            gain,
            statistics: self.statistics_demand(),
        }
    }

    /// Статистика кадра слота (собрана после fence): замер и приём его контроллером.
    pub fn ingest_statistics(
        &mut self,
        stream: ExposureStream,
        frame: AwbFrameIdentity,
        facts: &ExposureFrameFacts,
        statistics: &AwbStatistics,
    ) {
        if !self.bound.as_ref().is_some_and(|bound| bound.id == stream) {
            self.counters.statistics_stale += 1;
            return;
        }
        self.submit_measurement(stream, frame, facts, statistics);
        // Готовые замеры прошлых кадров забираем на КАЖДОМ пути, в том числе при раннем выходе (нет пары, область
        // недоступна, исполнитель занят): иначе оценка запаздывала бы на серию кадров без пары.
        self.collect_measurements(stream);
    }

    /// Ставит замер кадра в очередь исполнителя (или учитывает, почему кадр пропущен).
    fn submit_measurement(
        &mut self,
        stream: ExposureStream,
        frame: AwbFrameIdentity,
        facts: &ExposureFrameFacts,
        statistics: &AwbStatistics,
    ) {
        let Some(frame_product) = facts.product else {
            self.controller
                .note_skipped_measurement(SkippedMeasurement::NoPairedExposure);
            return;
        };
        let area = MeteredArea::for_point(self.directive.metering);
        self.ensure_weights(
            WeightsKey {
                stream,
                crop: facts.crop,
                area,
            },
            statistics,
        );
        let Some(CachedWeights {
            weights: Ok(request_weights),
            ..
        }) = &self.weights
        else {
            self.counters.skipped_area_unavailable += 1;
            return;
        };
        let request = ExposureMeteringRequest {
            frame: FrameTag {
                stream_generation: stream.0,
                sequence: frame.sequence,
                sensor_timestamp_nanos: frame.sensor_timestamp_nanos,
            },
            area,
            statistics,
            weights: request_weights,
            luminance: facts.luminance,
            frame_product,
            target: self.target,
            phase: self.controller.measurement_phase(),
        };
        if self.meter.submit(request).is_err() {
            self.controller.note_skipped_measurement(SkippedMeasurement::MeterBusy);
        }
    }

    /// Снимок диагностики.
    pub fn diagnostics(&self) -> AutoExposureDiagnostics {
        let counters = self.controller.counters();
        AutoExposureDiagnostics {
            directive_generation: self.applied_generation.map(|generation| generation.0),
            authority: self.directive.authority,
            metered_area: MeteredArea::for_point(self.directive.metering),
            target_log2_seconds_iso: self.controller.target_log2(),
            observed_scene_log2_seconds_iso: self.controller.last_observed_log2(),
            display_gain_ev: self.last_gain.ev(),
            convergence: self.last_proposal.map(|proposal| proposal.convergence),
            quality: self.last_proposal.map(|proposal| proposal.quality),
            last_proposal: self.last_proposal,
            counters: AutoExposureCounters {
                accepted: counters.accepted,
                failed: counters.failed,
                rejected_foreign_area: counters.rejected_foreign_area,
                rejected_stale: counters.rejected_stale,
                ignored_manual: counters.ignored_manual,
                ignored_holding: counters.ignored_holding,
                skipped_no_paired_exposure: counters.skipped_no_paired_exposure,
                skipped_meter_busy: counters.skipped_meter_busy,
                skipped_area_unavailable: self.counters.skipped_area_unavailable,
                statistics_stale: self.counters.statistics_stale,
                proposals_published: counters.proposals_published,
                cold_start_statistics_frames: self.counters.cold_start_statistics_frames,
                frames_with_residual_gain: self.counters.frames_with_residual_gain,
            },
        }
    }

    // ---- внутреннее ----

    /// Свежее указание движка (если появилось): применяется один раз на поколение. Смена области замера
    /// начинает холодный старт заново (оценка прежней области про другое место).
    fn refresh_directive(&mut self) {
        let Some((generation, directive)) = self.directives.newer_than(self.applied_generation) else {
            return;
        };
        let area_before = MeteredArea::for_point(self.directive.metering);
        self.controller.set_directive(directive);
        self.directive = directive;
        self.applied_generation = Some(generation);
        if MeteredArea::for_point(directive.metering) != area_before {
            self.restart_cold_start();
        }
        // Раз на смену указания, не на кадр.
        tracing::info!(target: LOG_TARGET, generation = generation.0, ?directive, "указание AE применено");
    }

    /// Подтяжка, округлённая до шага `residual_gain_step_ev`: см. поле конфига (ключ кэша параметров ISP).
    /// Подтяжка меньше половины шага становится ровно `UNITY`.
    fn quantized(&self, gain: ExposureGain) -> ExposureGain {
        let step = self.config.residual_gain_step_ev;
        // Шаг проверен конфигом (конечный, > 0), подтяжка ограничена контроллером: округление конечно.
        let steps = (gain.ev() / step).round();
        // Округление не должно выводить подтяжку за предел контроллера (шаг крупнее предела: 0,3 при 0,5 дало бы 0,6).
        let limit = self.config.controller.max_residual_ev;
        ExposureGain::from_ev((steps * step).clamp(-limit, limit)).unwrap_or(ExposureGain::UNITY)
    }

    /// Привязывает к потоку кадра; новый поток сбрасывает контроллер и начинает холодный старт.
    fn bind_stream(&mut self, passport: &RawFramePassport) -> ExposureStream {
        if let Some(bound) = &self.bound
            && Arc::ptr_eq(&bound.passport, &passport.stream)
        {
            return bound.id;
        }
        self.next_stream_id += 1;
        let id = ExposureStream(self.next_stream_id);
        // Замеры старого потока исполнитель мог ещё не отдать: они чужие.
        while self.meter.poll().is_some() {}
        self.controller.begin_stream();
        // Предложение прежнего потока в диагностике нового не показываем.
        self.last_proposal = None;
        self.weights = None;
        self.restart_cold_start();
        tracing::info!(target: LOG_TARGET, stream = id.0, "новый поток AE");
        self.bound = Some(BoundStream {
            passport: Arc::clone(&passport.stream),
            id,
        });
        id
    }

    fn restart_cold_start(&mut self) {
        self.has_estimate = false;
        self.cold_start_frames = 0;
        self.area_unavailable = false;
    }

    /// Чего AE хочет от статистики: «каждый кадр», пока нет первой оценки, но не дольше предела.
    fn statistics_demand(&mut self) -> StatisticsDemand {
        let manual = self.directive.authority == ExposureAuthority::Manual;
        let waiting = !self.has_estimate && !self.area_unavailable && !manual;
        if waiting && self.cold_start_frames < self.config.cold_start_every_frame_limit {
            self.cold_start_frames += 1;
            self.counters.cold_start_statistics_frames += 1;
            StatisticsDemand::EveryFrame
        } else {
            StatisticsDemand::WhenScheduled
        }
    }

    /// Гарантирует веса под поток, кадрирование и область: пересобираются только при смене любого из них.
    /// Неудача тоже кэшируется (иначе её пересчитывали бы на каждом кадре).
    fn ensure_weights(&mut self, key: WeightsKey, statistics: &AwbStatistics) {
        if self.weights.as_ref().is_some_and(|cached| cached.key == key) {
            return;
        }
        let built = self.build_weights(key.crop, statistics);
        match &built {
            Ok(_) => self.area_unavailable = false,
            Err(failure) => {
                self.area_unavailable = matches!(failure, WeightsUnavailable::RegionOutsideGrid);
                tracing::warn!(target: LOG_TARGET, ?failure, area = ?key.area, "область замера AE не построена: кадры пропускаются");
            }
        }
        self.weights = Some(CachedWeights { key, weights: built });
    }

    fn build_weights(
        &self,
        crop: PixelRect,
        statistics: &AwbStatistics,
    ) -> Result<MeteringWeights, WeightsUnavailable> {
        let Some(bound) = &self.bound else {
            return Err(WeightsUnavailable::Failed(MeteringWeightsError::InvalidPlacement));
        };
        let placement = tile_placement(&bound.passport, crop).map_err(WeightsUnavailable::Failed)?;
        let area = match self.directive.metering {
            PointOfInterest::WholeFrame => MeteringArea::Matrix,
            PointOfInterest::Point { region, .. } => MeteringArea::Spot(region),
        };
        MeteringWeights::build(statistics.grid(), placement, area, &self.config.metering).map_err(|error| match error {
            MeteringWeightsError::RegionOutsideGrid => WeightsUnavailable::RegionOutsideGrid,
            other => WeightsUnavailable::Failed(other),
        })
    }

    /// Забирает готовые замеры у исполнителя и отдаёт контроллеру.
    fn collect_measurements(&mut self, current: ExposureStream) {
        while let Some(done) = self.meter.poll() {
            if done.frame.stream_generation != current.0 {
                self.counters.statistics_stale += 1;
                continue;
            }
            if self.controller.ingest(&done) == IngestOutcome::Accepted {
                self.has_estimate = true;
            }
        }
    }
}

#[cfg(test)]
#[path = "gain_tests.rs"]
mod gain_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
