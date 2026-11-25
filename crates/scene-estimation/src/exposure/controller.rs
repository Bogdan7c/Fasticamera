//! Контроллер цели AE на потоке рендера: оценки сцены + указание движка + время кадров → сглаженная
//! цель «выдержка × ISO», предложение движку и остаточная подтяжка ISP.
//!
//! Все величины считаются в `log2` от произведения (1 единица = 1 EV). Это CPU-скаляры: на кадр
//! несколько умножений.
//!
//! # Время и сглаживание
//! Цель идёт к оценке сцены экспонентой с постоянной времени профиля (`Photo` быстро, `Video`
//! медленно). Шаг считается по меткам СЕНСОРА показанных кадров ([`SensorTimestamp`]); первый кадр,
//! метки не по порядку и нулевой шаг дают `dt = 0`, слишком большой шаг обрезается
//! `max_step_seconds`. Экспонента не перескакивает цель, поэтому перерегулирования нет.
//!
//! # Холодный старт
//! После [`AutoExposureController::begin_stream`] цели нет. Первая принятая оценка ставится цели
//! сразу, без сглаживания: сглаживать не от чего, любая стартовая точка была бы выдумкой.
//!
//! # Видео
//! «Мёртвая зона» с гистерезисом: цель стоит, пока ошибка меньше `video_dead_band_start_ev`; начав
//! движение, идёт до ошибки меньше `video_dead_band_stop_ev`. Это убирает «дыхание» от шума замера.
//!
//! # Блокировка
//! Запрошенная блокировка сначала досчитывает замер в области до `Settled` (или до
//! `lock_settle_timeout`), на это время цель идёт со скоростью `Photo`, затем цель замораживается
//! (`Holding`). Снятие блокировки снова включает слежение.

use camera_contract::auto_exposure::{
    AppliedExposure, CameraTag, ExposureAuthority, ExposureDirective, ExposureProduct, ExposureProposal,
    MeasurementQuality, MeteredArea, ProposalConvergence,
};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::three_a::{AeAfLock, ThreeAProfile};
use isp_model::params::ExposureGain;

use super::config::ExposureControllerConfig;
use super::measurement::MeasurementPhase;
use super::meter::CompletedExposureMeasurement;
use super::publication::{ProposalCandidate, ProposalPublisher};
use crate::error::ConfigError;

/// Что стало с замером при приёме.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    /// Оценка сцены обновлена.
    Accepted,
    /// Замера нет (исполнитель вернул ошибку).
    MeasurementFailed,
    /// Замер сделан по другой области, чем в текущем указании.
    RejectedForeignArea,
    /// Замер старше уже принятого.
    RejectedStale,
    /// Экспозиция ручная: замер ничего не решает.
    IgnoredManual,
    /// Цель заморожена блокировкой.
    IgnoredHolding,
}

/// Почему кадр статистики не дошёл до замера (учёт в [`ExposureCounters`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedMeasurement {
    /// У кадра нет результата с выдержкой и ISO (`FrameCaptureMeta::Paired` отсутствует).
    NoPairedExposure,
    /// Исполнитель замера занят (`SubmitError::Busy`).
    MeterBusy,
}

/// Счётчики решений (в диагностику).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExposureCounters {
    /// Замеров принято.
    pub accepted: u64,
    /// Замеров без результата (ошибка исполнителя).
    pub failed: u64,
    /// Замеров чужой области отброшено.
    pub rejected_foreign_area: u64,
    /// Устаревших замеров отброшено.
    pub rejected_stale: u64,
    /// Замеров проигнорировано ручной экспозицией.
    pub ignored_manual: u64,
    /// Замеров проигнорировано блокировкой.
    pub ignored_holding: u64,
    /// Кадров пропущено: нет парной экспозиции.
    pub skipped_no_paired_exposure: u64,
    /// Кадров пропущено: исполнитель занят.
    pub skipped_meter_busy: u64,
    /// Предложений опубликовано.
    pub proposals_published: u64,
}

/// Последняя принятая оценка сцены для области замера.
#[derive(Debug, Clone, Copy)]
struct SceneEstimate {
    log2_product: f64,
    quality: MeasurementQuality,
}

/// Фаза блокировки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockPhase {
    /// Слежение (блокировки нет).
    Following,
    /// Блокировка запрошена: досчитываем до `Settled`; срок задаётся на первом кадре.
    Settling { deadline: Option<SensorTimestamp> },
    /// Цель заморожена.
    Holding,
}

/// Контроллер цели AE (один на сессию показа; переживает смену потока).
#[derive(Debug, Clone)]
pub struct AutoExposureController {
    config: ExposureControllerConfig,
    directive: ExposureDirective,
    area: MeteredArea,
    smoothed_log2: Option<f64>,
    /// Ожидаемая цель нового потока до его первого замера (log2 от с·ISO): перенесена движком с прошлой
    /// камеры (решения 118, 126). Нужна ТОЛЬКО для подтяжки показа и проверки правдоподобия яркости; в
    /// предложения движку не попадает и первым же принятым замером вытесняется (см. [`Self::expect_target`]).
    expected_log2: Option<f64>,
    scene: Option<SceneEstimate>,
    last_measured_frame: Option<SensorTimestamp>,
    /// Была ли в этом потоке хоть одна надёжная (`Measured`) оценка: до неё замер идёт в фазе холодного старта.
    reliable_estimate_seen: bool,
    last_advance: Option<SensorTimestamp>,
    moving: bool,
    /// Гистерезис `Settled`: вошли в допуск и ещё не вышли за `unsettle_tolerance_ev`.
    settled_latched: bool,
    /// После запроса блокировки пришёл хотя бы один свежий замер (старая оценка не замораживается).
    fresh_since_lock: bool,
    lock: LockPhase,
    publisher: ProposalPublisher,
    counters: ExposureCounters,
    /// Последний сырой замер своей области — только для диагностики (08c): обновляется и под
    /// блокировкой, и в ручном режиме, когда оценка сцены замер не принимает. На решения не влияет.
    last_observed_log2: Option<f64>,
}

impl AutoExposureController {
    /// Контроллер с проверенным конфигом и указанием по умолчанию.
    pub fn new(config: ExposureControllerConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let directive = ExposureDirective::DEFAULT;
        Ok(Self {
            publisher: ProposalPublisher::new(&config),
            config,
            area: MeteredArea::for_point(directive.metering),
            lock: lock_phase_for(directive.lock),
            directive,
            smoothed_log2: None,
            expected_log2: None,
            scene: None,
            last_measured_frame: None,
            reliable_estimate_seen: false,
            last_advance: None,
            moving: false,
            settled_latched: false,
            fresh_since_lock: false,
            counters: ExposureCounters::default(),
            last_observed_log2: None,
        })
    }

    /// Новое указание движка. Смена области сбрасывает оценку сцены (она про другое место);
    /// смена блокировки запускает или снимает фазу блокировки.
    pub fn set_directive(&mut self, directive: ExposureDirective) {
        let area = MeteredArea::for_point(directive.metering);
        if area != self.area {
            self.area = area;
            self.scene = None;
            self.moving = false;
            self.settled_latched = false;
        }
        if directive.lock != self.directive.lock {
            self.lock = lock_phase_for(directive.lock);
            self.moving = false;
            self.fresh_since_lock = false;
        }
        if directive.authority == ExposureAuthority::Manual {
            // Замеры в ручном режиме игнорируются, поэтому оценка к возврату в авто устареет:
            // забываем её, цель стоит до первого свежего замера.
            self.scene = None;
            self.moving = false;
            self.settled_latched = false;
        }
        self.directive = directive;
    }

    /// Новый поток камеры: цели нет, первая надёжная оценка встанет сразу. Указание и счётчики остаются.
    pub fn begin_stream(&mut self) {
        self.smoothed_log2 = None;
        self.expected_log2 = None;
        self.scene = None;
        self.last_measured_frame = None;
        self.reliable_estimate_seen = false;
        self.last_advance = None;
        self.moving = false;
        self.settled_latched = false;
        self.fresh_since_lock = false;
        self.lock = lock_phase_for(self.directive.lock);
        self.publisher.reset();
        self.last_observed_log2 = None;
    }

    /// Ожидаемая экспозиция нового потока, пока своего замера у него нет (`None` — не ожидаем ничего).
    ///
    /// Подтяжка показа ([`Self::display_gain`]) и [`Self::display_target_log2`] берут её вместо цели, пока
    /// собственной цели потока нет. Предложения движку она НЕ порождает: ожидание — не измерение.
    /// Первый принятый замер ставит собственную цель, и ожидание перестаёт участвовать.
    /// [`Self::begin_stream`] ожидание сбрасывает: его выставляют заново после привязки потока.
    pub fn expect_target(&mut self, target: Option<ExposureProduct>) {
        self.expected_log2 = target.map(|product| product.seconds_iso().log2());
    }

    /// Учитывает кадр, который не дошёл до замера.
    pub fn note_skipped_measurement(&mut self, reason: SkippedMeasurement) {
        match reason {
            SkippedMeasurement::NoPairedExposure => self.counters.skipped_no_paired_exposure += 1,
            SkippedMeasurement::MeterBusy => self.counters.skipped_meter_busy += 1,
        }
    }

    /// Принимает готовый замер как оценку сцены для области указания.
    pub fn ingest(&mut self, completed: &CompletedExposureMeasurement) -> IngestOutcome {
        let measurement = match &completed.outcome {
            Ok(measurement) => measurement,
            Err(_) => {
                self.counters.failed += 1;
                return IngestOutcome::MeasurementFailed;
            }
        };
        if completed.area != self.area {
            self.counters.rejected_foreign_area += 1;
            return IngestOutcome::RejectedForeignArea;
        }
        // Диагностика видит сцену всегда, даже когда оценка ниже замер отбросит (блокировка, ручной
        // режим): только так приёмка доказывает «сцена сменилась, а экспозиция стоит».
        self.last_observed_log2 = Some(measurement.required_product.seconds_iso().log2());
        if self.directive.authority == ExposureAuthority::Manual {
            self.counters.ignored_manual += 1;
            return IngestOutcome::IgnoredManual;
        }
        if self.lock == LockPhase::Holding {
            self.counters.ignored_holding += 1;
            return IngestOutcome::IgnoredHolding;
        }
        let frame_time = SensorTimestamp::from_nanos(completed.frame.sensor_timestamp_nanos);
        if self.last_measured_frame.is_some_and(|last| frame_time < last) {
            self.counters.rejected_stale += 1;
            return IngestOutcome::RejectedStale;
        }
        let log2_product = measurement.required_product.seconds_iso().log2();
        self.scene = Some(SceneEstimate {
            log2_product,
            quality: measurement.quality,
        });
        self.last_measured_frame = Some(frame_time);
        self.reliable_estimate_seen |= measurement.quality == MeasurementQuality::Measured;
        self.fresh_since_lock = true;
        // Холодный старт: сглаживать не от чего, цель встаёт сразу.
        self.smoothed_log2.get_or_insert(log2_product);
        self.counters.accepted += 1;
        IngestOutcome::Accepted
    }

    /// Шаг цели на показанный кадр с меткой `sensor_timestamp`.
    pub fn advance(&mut self, sensor_timestamp: SensorTimestamp) {
        let elapsed_seconds = self.elapsed_seconds(sensor_timestamp);
        if self.last_advance.is_none_or(|last| sensor_timestamp > last) {
            self.last_advance = Some(sensor_timestamp);
        }
        if self.directive.authority == ExposureAuthority::Manual {
            return;
        }
        self.update_lock_phase(sensor_timestamp);
        if self.lock == LockPhase::Holding {
            return;
        }
        let (Some(current), Some(scene)) = (self.smoothed_log2, self.scene) else {
            return;
        };
        let error = scene.log2_product - current;
        if self.directive.profile == ThreeAProfile::Video && self.lock == LockPhase::Following {
            self.update_dead_band(error.abs());
            if !self.moving {
                return;
            }
        }
        let blend = 1.0 - (-elapsed_seconds / self.time_constant_seconds()).exp();
        self.smoothed_log2 = Some(current + error * blend);
    }

    /// Предложение движку, если его пора публиковать; `None` — пока нечего или рано.
    ///
    /// `measured_on` — камера кадра, по которому идёт замер: предложение несёт её, чтобы движок не принял
    /// замер другой (одновременно стримящей) камеры.
    pub fn proposal(&mut self, sensor_timestamp: SensorTimestamp, measured_on: CameraTag) -> Option<ExposureProposal> {
        if self.directive.authority == ExposureAuthority::Manual {
            return None;
        }
        let (current, scene) = (self.smoothed_log2?, self.scene?);
        let measured_frame = self.last_measured_frame?;
        let candidate = ProposalCandidate {
            target_log2: current,
            convergence: self.convergence(current, scene),
            quality: scene.quality,
            area: self.area,
        };
        // Цель — экспонента конечного числа в разумных пределах, ошибки не бывает; если бы была — предложения нет.
        let target = ExposureProduct::from_seconds_iso(current.exp2()).ok()?;
        if !self.publisher.try_publish(sensor_timestamp, candidate) {
            return None;
        }
        self.counters.proposals_published += 1;
        Some(ExposureProposal {
            measured_frame,
            measured_on,
            target,
            metered_area: candidate.area,
            convergence: candidate.convergence,
            quality: candidate.quality,
        })
    }

    /// Остаточная подтяжка ISP для кадра с произведением `frame_product` (из его результата).
    ///
    /// `Manual`: `D = applied.product`; иначе `D = clamp(T · 2^bias, достижимое)`, где `T` — сглаженная
    /// цель, «достижимое» — `applied.reach`. `k = D / A`, `|log2 k| ≤ max_residual_ev`. Нет данных (кадра, указания, цели) — `UNITY`.
    pub fn display_gain(&self, frame_product: Option<ExposureProduct>) -> ExposureGain {
        let (Some(frame_product), Some(applied)) = (frame_product, self.directive.applied) else {
            return ExposureGain::UNITY;
        };
        let Some(wanted_log2) = self.wanted_display_log2(applied) else {
            return ExposureGain::UNITY;
        };
        let limit = f64::from(self.config.max_residual_ev);
        let residual_ev = (wanted_log2 - frame_product.seconds_iso().log2()).clamp(-limit, limit);
        // Конечное число в пределах ±max_residual_ev: `from_ev` не может отказать; на случай NaN подтяжки нет.
        ExposureGain::from_ev(residual_ev as f32).unwrap_or(ExposureGain::UNITY)
    }

    /// Фаза замера для следующих кадров: холодный старт, пока в потоке не было надёжной оценки.
    pub fn measurement_phase(&self) -> MeasurementPhase {
        if self.reliable_estimate_seen {
            MeasurementPhase::Established
        } else {
            MeasurementPhase::ColdStart
        }
    }

    /// Счётчики решений.
    pub fn counters(&self) -> ExposureCounters {
        self.counters
    }

    /// Сглаженная цель в EV относительно произведения 1 с·ISO (для диагностики и тестов).
    pub fn target_log2(&self) -> Option<f64> {
        self.smoothed_log2
    }

    /// Цель для ПОКАЗА, log2 от с·ИСО: собственная сглаженная, а пока её нет — ожидаемая (перенесённая).
    /// `None` — нет ни той, ни другой.
    pub fn display_target_log2(&self) -> Option<f64> {
        self.smoothed_log2.or(self.expected_log2)
    }

    /// Показ сейчас идёт по ожидаемой цели, а не по собственной (замера в потоке ещё не было).
    pub fn is_display_target_expected(&self) -> bool {
        self.smoothed_log2.is_none() && self.expected_log2.is_some()
    }

    /// Последний сырой замер своей области, log2 от с·ISO, — без сглаживания и даже под
    /// блокировкой или в ручном режиме (только диагностика; `None` — замеров в этом потоке не было).
    pub fn last_observed_log2(&self) -> Option<f64> {
        self.last_observed_log2
    }

    /// Ждём ли мы сейчас заморозки цели или уже держим её.
    pub fn is_holding(&self) -> bool {
        self.lock == LockPhase::Holding
    }

    /// `D` в log2: что должно быть на экране.
    fn wanted_display_log2(&self, applied: AppliedExposure) -> Option<f64> {
        match self.directive.authority {
            ExposureAuthority::Manual => Some(applied.product.seconds_iso().log2()),
            ExposureAuthority::Automatic | ExposureAuthority::SemiAutomatic => {
                // Цель с компенсацией, срезанная до достижимого запросом. Достижимость — свойство
                // указания (не кадра), поэтому не отстаёт от цели: на ступеньке за предел сенсора подтяжка
                // не тянется к недостижимому и не перелетает, когда запрос догонит. Нехватку
                // `applied.shortfall_ev` НЕ берём: она посчитана для цели прошлого поколения.
                let requested_log2 = self.display_target_log2()? + f64::from(self.directive.bias.ev());
                Some(applied.reach.clamp_log2(requested_log2))
            }
        }
    }

    fn convergence(&mut self, current: f64, scene: SceneEstimate) -> ProposalConvergence {
        if self.lock == LockPhase::Holding {
            return ProposalConvergence::Holding;
        }
        let error = (scene.log2_product - current).abs();
        let reliable = scene.quality == MeasurementQuality::Measured;
        // Гистерезис: внутрь допуска входим при `settled_tolerance_ev`, выходим при `unsettle_tolerance_ev`.
        if !reliable || error > f64::from(self.config.unsettle_tolerance_ev) {
            self.settled_latched = false;
        } else if error <= f64::from(self.config.settled_tolerance_ev) {
            self.settled_latched = true;
        }
        // Покой в мёртвой зоне видео — тоже Settled, но не во время досчёта блокировки (там `moving` не ведётся).
        let resting_in_dead_band =
            self.directive.profile == ThreeAProfile::Video && self.lock == LockPhase::Following && !self.moving;
        if reliable && (self.settled_latched || resting_in_dead_band) {
            ProposalConvergence::Settled
        } else {
            ProposalConvergence::Searching
        }
    }

    /// Шаг времени с прошлого кадра: не по порядку и первый кадр — 0, большой — не больше предела.
    fn elapsed_seconds(&self, now: SensorTimestamp) -> f64 {
        let elapsed = self
            .last_advance
            .and_then(|last| now.duration_since(last))
            .map_or(0.0, |duration| duration.as_secs_f64());
        elapsed.min(f64::from(self.config.max_step_seconds))
    }

    /// Фаза блокировки: `Settling` → `Holding`, когда замер сошёлся или вышло время.
    fn update_lock_phase(&mut self, now: SensorTimestamp) {
        let LockPhase::Settling { deadline } = self.lock else {
            return;
        };
        let timeout_nanos = (f64::from(self.config.lock_settle_timeout_seconds) * 1.0e9) as i64;
        let deadline =
            deadline.unwrap_or_else(|| SensorTimestamp::from_nanos(now.as_nanos().saturating_add(timeout_nanos)));
        let settled = match (self.smoothed_log2, self.scene) {
            (Some(current), Some(scene)) if self.fresh_since_lock => {
                scene.quality == MeasurementQuality::Measured
                    && (scene.log2_product - current).abs() <= f64::from(self.config.settled_tolerance_ev)
            }
            _ => false,
        };
        self.lock = if settled || now >= deadline {
            LockPhase::Holding
        } else {
            LockPhase::Settling {
                deadline: Some(deadline),
            }
        };
    }

    /// Гистерезис мёртвой зоны видео по модулю ошибки.
    fn update_dead_band(&mut self, error_ev: f64) {
        if self.moving {
            self.moving = error_ev >= f64::from(self.config.video_dead_band_stop_ev);
        } else {
            self.moving = error_ev >= f64::from(self.config.video_dead_band_start_ev);
        }
    }

    /// Постоянная времени: при досчёте блокировки — как у фото, иначе по профилю.
    fn time_constant_seconds(&self) -> f64 {
        let photo = f64::from(self.config.photo_time_constant_seconds);
        let video = f64::from(self.config.video_time_constant_seconds);
        if matches!(self.lock, LockPhase::Settling { .. }) {
            return photo;
        }
        match self.directive.profile {
            ThreeAProfile::Photo => photo,
            ThreeAProfile::Video => video,
        }
    }
}

fn lock_phase_for(lock: AeAfLock) -> LockPhase {
    match lock {
        AeAfLock::Unlocked => LockPhase::Following,
        AeAfLock::Locked => LockPhase::Settling { deadline: None },
    }
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "controller_reach_tests.rs"]
mod reach_tests;

#[cfg(test)]
#[path = "controller_expected_tests.rs"]
mod expected_tests;

#[cfg(test)]
#[path = "controller_lock_tests.rs"]
mod lock_tests;
