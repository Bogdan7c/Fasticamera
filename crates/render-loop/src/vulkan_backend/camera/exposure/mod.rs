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

mod carried;
mod placement;
mod preflash;
mod residual_gain;
mod reveal_level;

use std::cell::Cell;
use std::sync::Arc;

use camera_contract::auto_exposure::{
    CameraTag, ExposureAuthority, ExposureDirective, ExposureProduct, ExposureProposal, ExposureProposalChannel,
    MeteredArea,
};
use camera_contract::flash::{FlashFramePlan, FrameLight, PreflashMetering};
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
use self::preflash::PreflashMeter;
pub use self::residual_gain::ResidualGainLimits;
use self::reveal_level::{DisplayedLevelInputs, displayed_level_error_ev};
use super::continuity::ExposureReadiness;
use super::flash_facts::AmbientLevel;
use super::frame_role::frame_light;
use super::statistics_schedule::StatisticsDemand;
use crate::config::{AutoExposureConfig, RevealExposureCheck};
use crate::viewfinder::{AutoExposureCounters, AutoExposureDiagnostics, AwbFrameIdentity};

/// Кадров «каждый кадр» после конца предвспышки: пауза (≈ 6 кадров) + разовый кадр (≈ 6) + запас.
const POST_FLASH_EVERY_FRAME_FRAMES: u32 = 30;
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
    /// Чем освещён кадр (по его результату): кадр со светодиодом даёт замер предвспышки, но не питает
    /// контроллер AE видоискателя.
    pub light: FrameLight,
}

impl ExposureFrameFacts {
    /// Факты кадра `passport`, обработанного параметрами `params` с кадрированием `crop`.
    pub fn of_frame(passport: &RawFramePassport, params: &IspParams, crop: PixelRect) -> Self {
        Self {
            product: frame_exposure_product(passport),
            luminance: params.scene_luminance_weights(),
            crop,
            light: frame_light(passport),
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
    /// Кадров, признанных готовыми по ожидаемой (перенесённой) экспозиции. `Cell`: `reveal_readiness` — запрос
    /// (`&self`), счёт — побочное наблюдение, не состояние решения.
    frames_ready_by_carried_exposure: Cell<u64>,
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
    /// Сколько ещё кадров после предвспышки мерить каждый кадр: покадровая лента вокруг кадра со вспышкой
    /// (на каком кадре на самом деле свет). Диагностика 14d, цена — статистика на полсекунды.
    post_flash_every_frame_left: u32,
    last_gain: ExposureGain,
    /// Произведение показанного кадра, для которого посчитана `last_gain`.
    last_frame_product: Option<ExposureProduct>,
    last_proposal: Option<ExposureProposal>,
    /// Объявления движка «чего ждать от нового потока», по камерам (см. `carried`).
    carried: carried::CarriedExpectations,
    /// Замеры предвспышки и факты для движка снимков (14d).
    preflash: PreflashMeter,
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
            post_flash_every_frame_left: 0,
            last_gain: ExposureGain::UNITY,
            last_frame_product: None,
            last_proposal: None,
            carried: carried::CarriedExpectations::default(),
            preflash: PreflashMeter::default(),
            counters: RunnerCounters::default(),
        })
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток.
    pub fn forget_stream(&mut self) {
        self.bound = None;
    }

    /// Готова ли яркость потока кадра `passport` к показу вместо удержанного кадра (сессия 13, смена объектива).
    ///
    /// Яркость «правдоподобна», когда AE уже измерил сцену ЭТОГО потока и показанный уровень кадра (произведение
    /// «выдержка × ISO» кадра + остаточная подтяжка) отличается от цели AE не больше чем на `max_error_ev`.
    /// Принятый замер сам по себе не доказывает правдоподобия: поток мог начаться с чёрной выдержки, а цель к
    /// ней ещё не сошлась. Цель берётся ДОСТИЖИМАЯ (срезана пределами потока): в тёмной сцене кадр на краю
    /// пределов уже готов, ждать недостижимого незачем.
    ///
    /// Ожидание НЕ включается там, где AE нечего измерять и ждать было бы зря весь срок удержания: проверка
    /// выключена конфигом, экспозиция ручная, область замера вне сетки статистики, у кадра нет спаренных выдержки
    /// и ISO. Вызывать после `plan_frame` этого кадра (подтяжка и произведение кадра уже посчитаны).
    pub fn reveal_readiness(
        &self,
        passport: &RawFramePassport,
        check: RevealExposureCheck,
        max_error_ev: f32,
    ) -> ExposureReadiness {
        let nothing_to_wait_for = check == RevealExposureCheck::Ignore
            || self.directive.authority == ExposureAuthority::Manual
            || self.area_unavailable
            || frame_exposure_product(passport).is_none();
        if nothing_to_wait_for {
            return ExposureReadiness::Ready;
        }
        // Собственного замера у потока ещё нет, но движок передал ожидаемую экспозицию для ЭТОЙ камеры
        // (`carried`): судим по ней той же формулой и тем же допуском.
        let judged_by_carried = !self.has_estimate && self.controller.is_display_target_expected();
        // Цели нет (ни замера, ни объявления для камеры кадра) или нечем сравнить — ждём цель; это причина,
        // отличная от «цель есть, но уровень не тот»: на телефоне их нужно различать (диагностика удержаний).
        let error_ev = if self.has_estimate || judged_by_carried {
            self.displayed_level_error_ev()
        } else {
            None
        };
        let Some(error_ev) = error_ev else {
            return ExposureReadiness::WaitingForTarget;
        };
        // Несравнимая ошибка (NaN) — тоже «уровень не тот», а не «готов»: годится только ошибка в допуске.
        let within_tolerance = error_ev.abs() <= f64::from(max_error_ev);
        if !within_tolerance {
            return ExposureReadiness::WaitingLevelOff;
        }
        if judged_by_carried {
            let counter = &self.counters.frames_ready_by_carried_exposure;
            counter.set(counter.get() + 1);
        }
        ExposureReadiness::Ready
    }

    /// Насколько показанный уровень кадра выше ДОСТИЖИМОЙ цели AE, EV (`None` — цели или произведения кадра нет).
    /// Что такое достижимая цель и почему пределам верим не всегда — в `reveal_level`.
    fn displayed_level_error_ev(&self) -> Option<f64> {
        let frame_log2 = self.last_frame_product?.seconds_iso().log2();
        let target_log2 = self.controller.display_target_log2()?;
        Some(displayed_level_error_ev(DisplayedLevelInputs {
            frame_log2,
            gain_ev: f64::from(self.last_gain.ev()),
            target_log2,
            bias_ev: f64::from(self.directive.bias.ev()),
            applied: self.directive.applied,
            // Меньше шага подтяжки глаз и квантование ISP различить не могут: это и есть допуск округления.
            reach_tolerance_ev: f64::from(self.config.residual_gain_step_ev),
        }))
    }

    /// План для нового кадра камеры (см. модуль). Вызывается до подготовки параметров ISP.
    pub fn plan_frame(&mut self, passport: &RawFramePassport) -> ExposureFramePlan {
        self.refresh_directive();
        let stream = self.bind_stream(passport);
        // Ожидаемая экспозиция нового потока (если движок передал её для камеры кадра) — до шага цели и подтяжки.
        self.controller.expect_target(self.carried.expected_target(passport));
        // Шаг цели и предложение — по метке сенсора показываемого кадра.
        self.controller.advance(passport.sensor_timestamp);
        if let Some(proposal) = self
            .controller
            .proposal(passport.sensor_timestamp, CameraTag::of(&passport.stream.camera))
        {
            self.proposals.publish(proposal);
            self.last_proposal = Some(proposal);
        }
        let frame_product = frame_exposure_product(passport);
        let gain = self.quantized(self.controller.display_gain(frame_product));
        if gain != ExposureGain::UNITY {
            self.counters.frames_with_residual_gain += 1;
        }
        self.last_gain = gain;
        self.last_frame_product = frame_product;
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
        } else {
            // Ответ придёт позже и без света кадра: запоминаем его по метке сенсора.
            self.preflash.note_submitted(frame.sensor_timestamp_nanos, facts.light);
        }
    }

    /// Новые планы кадра со вспышкой из указаний движка (для движка снимков; список становится пустым).
    pub fn drain_flash_plans(&mut self) -> Vec<FlashFramePlan> {
        self.preflash.drain_plans()
    }

    /// Новые уровни света сцены без вспышки по кадрам (для проверки «кадр со вспышкой освещён»;
    /// список становится пустым).
    pub fn drain_ambient_levels(&mut self) -> Vec<AmbientLevel> {
        self.preflash.drain_ambient_levels()
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
            displayed_frame_log2_seconds_iso: self.last_frame_product.map(|product| product.seconds_iso().log2()),
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
                frames_ready_by_carried_exposure: self.counters.frames_ready_by_carried_exposure.get(),
                preflash_samples_published: self.preflash.samples_published,
                preflash_not_computable: self.preflash.not_computable,
                measurements_withheld_for_led_light: self.preflash.withheld_from_controller,
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
        let decision_changed = changes_decision(&self.directive, &directive);
        self.controller.set_directive(directive);
        let preflash_ended = matches!(self.directive.flash.preflash, PreflashMetering::Active { .. })
            && !matches!(directive.flash.preflash, PreflashMetering::Active { .. });
        if preflash_ended {
            self.post_flash_every_frame_left = POST_FLASH_EVERY_FRAME_FRAMES;
        }
        self.directive = directive;
        self.applied_generation = Some(generation);
        // Объявление ожидаемой экспозиции запоминаем по камере сразу: следующее указание может его перезаписать
        // раньше, чем рендер покажет первый кадр этой камеры.
        self.carried.observe(&directive);
        self.preflash.observe_plan(directive.flash.planned);
        if MeteredArea::for_point(directive.metering) != area_before {
            self.restart_cold_start();
        }
        // Решение (область, блокировка, профиль, хозяин, EV) меняется редко — это info. Обратная связь
        // «что применено на сенсоре» (`applied`) обновляет указание почти каждый результат (≈ 15 раз в секунду):
        // на info она забивала буфер logcat и вытесняла строки старта (найдено в 12b), поэтому — debug.
        if decision_changed {
            tracing::info!(target: LOG_TARGET, generation = generation.0, ?directive, "указание AE применено");
        } else {
            tracing::debug!(target: LOG_TARGET, generation = generation.0, applied = ?directive.applied, "обратная связь AE обновлена");
        }
    }

    /// Подтяжка, округлённая до шага `residual_gain_step_ev`: см. поле конфига (ключ кэша параметров ISP).
    fn quantized(&self, gain: ExposureGain) -> ExposureGain {
        ResidualGainLimits::of(&self.config).quantize_ev(gain.ev())
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
        self.preflash.reset_for_new_stream();
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
        // Предвспышка: движку нужен замер КАЖДОГО кадра (и без фонарика, и с ним), чтобы поймать кадры с фонариком.
        if matches!(self.directive.flash.preflash, PreflashMetering::Active { .. }) {
            return StatisticsDemand::EveryFrame;
        }
        if self.post_flash_every_frame_left > 0 {
            self.post_flash_every_frame_left -= 1;
            return StatisticsDemand::EveryFrame;
        }
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
            let Some(bound) = self.bound.as_ref() else { continue };
            let outcome =
                self.preflash
                    .complete(&done, &bound.passport, self.target, self.config.metering.key_offset_ev);
            if let Some(samples) = outcome.publish {
                // Толкаем движок только во время предвспышки: вне её замеры «без фонарика» лишь копятся
                // в ящике, движок прочитает их сам в начале предвспышки.
                if matches!(self.directive.flash.preflash, PreflashMetering::Active { .. }) {
                    self.proposals.publish_preflash(samples);
                } else {
                    self.proposals.store_preflash_quietly(samples);
                }
            }
            if !outcome.light.feeds_viewfinder_smoothing() {
                // Кадр освещён светодиодом: замер ушёл вспышке, а видоискателю нельзя — потянул бы экспозицию к свету LED.
                self.preflash.withheld_from_controller += 1;
                continue;
            }
            if self.controller.ingest(&done) == IngestOutcome::Accepted {
                self.has_estimate = true;
            }
        }
    }
}

/// Изменилось ли в указании AE РЕШЕНИЕ движка, а не только обратная связь `applied` (см. `refresh_directive`).
fn changes_decision(previous: &ExposureDirective, next: &ExposureDirective) -> bool {
    let ExposureDirective {
        metering,
        lock,
        profile,
        authority,
        bias,
        applied: _,
        stream_start,
        flash,
    } = *next;
    previous.metering != metering
        || previous.lock != lock
        || previous.profile != profile
        || previous.authority != authority
        || previous.bias != bias
        || previous.stream_start != stream_start
        || previous.flash != flash
}

#[cfg(test)]
#[path = "carried_tests.rs"]
mod carried_tests;
#[cfg(test)]
#[path = "gain_tests.rs"]
mod gain_tests;
#[cfg(test)]
#[path = "preflash_tests.rs"]
mod preflash_tests;
#[cfg(test)]
pub(in crate::vulkan_backend::camera) mod test_support;
#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
