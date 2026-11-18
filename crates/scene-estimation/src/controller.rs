//! Решение AWB на CPU: указание владельца 3A + оценки света + время кадров → белая точка для ISP.
//!
//! # Режимы (`WhiteBalanceDirective`, решение 64)
//! * **Auto** — цель — последняя принятая оценка света; белая плавно идёт к ней со скоростью профиля
//!   (`Photo` быстрее, `Video` медленнее). Первая принятая оценка на потоке БЕЗ прежней белой принимается
//!   прыжком (сглаживать не от чего: стартовая точка — выдумка).
//! * **Locked** — белая замораживается там, где была. Если белой ещё нет (блокировка на холодном старте),
//!   принимается ПЕРВАЯ достаточно уверенная оценка и тут же замораживается («поймать и держать»).
//! * **Manual** — цель — ручная точка (`температура + Duv`), быстро (`manual_time_constant_seconds`); на
//!   потоке без белой — прыжок сразу. Точка вне таблицы камеры прижимается к ней, и прижим виден в
//!   [`ManualLimit`] (камера-core таблицы не видит, поэтому отчёт «ограничено до» рождается здесь).
//!
//! # Время
//! Шаг сглаживания считается по меткам СЕНСОРА показанных кадров, а не по часам стены: оценки приходят
//! редко (статистика прореживается), а сглаживание идёт на КАЖДЫЙ показанный кадр. Первый кадр потока и
//! неположительный шаг дают `dt = 0`; слишком большой скачок (пауза, смена потока) обрезается до
//! `max_step_seconds`.
//!
//! # Слабая оценка
//! Оценка с уверенностью или числом плиток ниже порога цель НЕ обновляет: белая держится (идёт к прежней
//! цели, если не дошла). Так сцена из одного цвета не уводит баланс.

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, WhitePointError, chromaticity_from_cct_duv};

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::three_a::{ThreeAProfile, WhiteBalanceDirective, WhiteBalanceMode};

use crate::config::SceneEstimationConfig;
use crate::error::ConfigError;
use crate::estimate::{EstimateOutcome, IlluminantEstimate};
use crate::smoother::{Report, ReportThresholds, WhiteSmoother, WhiteState};

const NANOS_PER_SECOND: f64 = 1.0e9;

/// Белая сцены в удобных видах.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneWhite {
    /// Цветность (то, что ходит между модулями и идёт в нейтраль камеры).
    pub chromaticity: Chromaticity,
    /// Температура и Duv (для диагностики и смены поколения).
    pub cct_duv: CctDuv,
}

/// Результат шага на кадр.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteStep {
    /// Сообщённая ISP белая; `None` — белой ещё нет (ждём первую оценку или ручную точку).
    pub white: Option<SceneWhite>,
    /// Белая изменилась заметно с прошлого шага: ISP надо пересчитать. `false` — прежние параметры.
    pub changed: bool,
}

/// Ручная точка ограничена таблицей камеры.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManualLimit {
    /// Что просили: температура, К.
    pub requested_kelvin: f64,
    /// Что применено: температура, К.
    pub applied_kelvin: f64,
    /// Что просили: Duv.
    pub requested_duv: f64,
    /// Что применено: Duv.
    pub applied_duv: f64,
}

/// Что стало с оценкой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IngestOutcome {
    /// Цель обновлена.
    Accepted,
    /// Серых плиток не нашлось: белая держится.
    HeldNoCandidates,
    /// Уверенность или число плиток ниже порога: белая держится.
    HeldLowConfidence {
        /// Уверенность оценки.
        confidence: f32,
        /// Принятых плиток.
        accepted_tiles: u32,
    },
    /// Режим «блокировка» и белая уже есть: оценка не нужна.
    IgnoredLocked,
}

/// Счётчики решений по оценкам (в диагностику).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControllerCounters {
    /// Оценок принято.
    pub accepted: u64,
    /// Оценок без кандидатов.
    pub held_no_candidates: u64,
    /// Оценок ниже порога уверенности.
    pub held_low_confidence: u64,
    /// Оценок проигнорировано блокировкой.
    pub ignored_locked: u64,
}

/// Ручная цель и то, чем она ограничена.
#[derive(Debug, Clone, Copy)]
struct ManualTarget {
    state: WhiteState,
    limit: Option<ManualLimit>,
}

/// Контроллер белой точки (один на сессию показа; переживает смену потока).
#[derive(Debug, Clone)]
pub struct WhiteBalanceController {
    config: SceneEstimationConfig,
    directive: WhiteBalanceDirective,
    auto_target: Option<WhiteState>,
    manual_target: Option<ManualTarget>,
    smoother: WhiteSmoother,
    last_timestamp_nanos: Option<i64>,
    counters: ControllerCounters,
}

impl WhiteBalanceController {
    /// Контроллер с указанием по умолчанию (`WhiteBalanceDirective::DEFAULT`).
    pub fn new(config: SceneEstimationConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        let thresholds = ReportThresholds {
            mired: config.controller.report_threshold_mired,
            duv: config.controller.report_threshold_duv,
        };
        Ok(Self {
            config,
            directive: WhiteBalanceDirective::DEFAULT,
            auto_target: None,
            manual_target: None,
            smoother: WhiteSmoother::new(thresholds),
            last_timestamp_nanos: None,
            counters: ControllerCounters::default(),
        })
    }

    /// Новое указание владельца 3A. Прежняя белая сохраняется (смена режима не обнуляет сцену).
    pub fn set_directive(&mut self, directive: WhiteBalanceDirective) {
        self.directive = directive;
        self.manual_target = match directive.mode {
            WhiteBalanceMode::Manual { setting } => Some(self.manual_target_for(setting)),
            WhiteBalanceMode::Auto | WhiteBalanceMode::Locked => None,
        };
    }

    /// Действующее указание.
    pub fn directive(&self) -> WhiteBalanceDirective {
        self.directive
    }

    /// Начался новый поток (другая камера): белая сцены сохраняется как стартовая, время кадров — с нуля.
    pub fn begin_stream(&mut self) {
        self.last_timestamp_nanos = None;
    }

    /// Известна ли белая сцены (есть от чего стартовать на новом потоке).
    pub fn has_scene_white(&self) -> bool {
        self.smoother.current().is_some()
    }

    /// Текущая белая в виде `(майред, Duv)`.
    pub fn current_state(&self) -> Option<WhiteState> {
        self.smoother.current()
    }

    /// Счётчики решений по оценкам.
    pub fn counters(&self) -> ControllerCounters {
        self.counters
    }

    /// Ручная точка вне таблицы камеры: что просили и что применено (`None` — ограничения нет или режим не ручной).
    pub fn manual_limit(&self) -> Option<ManualLimit> {
        match self.directive.mode {
            WhiteBalanceMode::Manual { .. } => self.manual_target.and_then(|target| target.limit),
            WhiteBalanceMode::Auto | WhiteBalanceMode::Locked => None,
        }
    }

    /// Восстанавливает белую сцены, сохранённую владельцем раньше (новая сессия показа, другая камера):
    /// стартовая точка без сглаживания. Если белая уже есть — ничего не меняет.
    pub fn restore_scene_white(&mut self, state: WhiteState) {
        if self.smoother.current().is_none() {
            self.smoother.snap_to(state);
        }
    }

    /// Берёт слабую оценку как «лучшую из доступных» для первого показа, если дольше ждать нельзя
    /// (конец ограниченного скрытия на холодном старте). Работает только пока белой ещё нет; верная
    /// уверенная оценка потом поведёт белую обычным путём (плавно, а не прыжком). Возвращает, принята ли.
    pub fn adopt_best_effort(&mut self, estimate: &IlluminantEstimate) -> bool {
        if self.smoother.current().is_some() {
            return false;
        }
        let state = WhiteState {
            mired: estimate.cct_duv.temperature.mired(),
            duv: estimate.cct_duv.duv.0,
        };
        self.smoother.snap_to(state);
        true
    }

    /// Принимает исход оценки (когда исполнитель её отдал).
    pub fn ingest(&mut self, outcome: &EstimateOutcome) -> IngestOutcome {
        let estimate = match outcome {
            EstimateOutcome::NoCandidates { .. } => {
                self.counters.held_no_candidates += 1;
                return IngestOutcome::HeldNoCandidates;
            }
            EstimateOutcome::Estimated(estimate) => estimate,
        };
        let controller = &self.config.controller;
        if estimate.confidence < controller.min_confidence || estimate.accepted_tiles < controller.min_accepted_tiles {
            self.counters.held_low_confidence += 1;
            return IngestOutcome::HeldLowConfidence {
                confidence: estimate.confidence,
                accepted_tiles: estimate.accepted_tiles,
            };
        }
        if self.directive.mode == WhiteBalanceMode::Locked && self.smoother.current().is_some() {
            self.counters.ignored_locked += 1;
            return IngestOutcome::IgnoredLocked;
        }
        let target = WhiteState {
            mired: estimate.cct_duv.temperature.mired(),
            duv: estimate.cct_duv.duv.0,
        };
        self.auto_target = Some(target);
        if self.smoother.current().is_none() {
            self.smoother.snap_to(target);
        }
        self.counters.accepted += 1;
        IngestOutcome::Accepted
    }

    /// Шаг на показанный кадр с меткой сенсора `sensor_timestamp_nanos`.
    pub fn advance(&mut self, sensor_timestamp_nanos: i64) -> Result<WhiteStep, WhitePointError> {
        let dt_seconds = self.step_seconds(sensor_timestamp_nanos);
        match self.directive.mode {
            WhiteBalanceMode::Auto => {
                if let Some(target) = self.auto_target {
                    let tau = match self.directive.profile {
                        ThreeAProfile::Photo => self.config.controller.photo_time_constant_seconds,
                        ThreeAProfile::Video => self.config.controller.video_time_constant_seconds,
                    };
                    self.smoother.approach(target, tau, dt_seconds);
                }
            }
            WhiteBalanceMode::Locked => {}
            WhiteBalanceMode::Manual { .. } => {
                if let Some(target) = self.manual_target {
                    if self.smoother.current().is_none() {
                        self.smoother.snap_to(target.state);
                    }
                    let tau = self.config.controller.manual_time_constant_seconds;
                    self.smoother.approach(target.state, tau, dt_seconds);
                }
            }
        }
        match self.smoother.report() {
            Report::NoWhiteYet => Ok(WhiteStep {
                white: None,
                changed: false,
            }),
            Report::Unchanged(state) => Ok(WhiteStep {
                white: Some(scene_white(state)?),
                changed: false,
            }),
            Report::Changed(state) => Ok(WhiteStep {
                white: Some(scene_white(state)?),
                changed: true,
            }),
        }
    }

    /// Шаг времени между показанными кадрами по меткам сенсора (см. модуль).
    fn step_seconds(&mut self, sensor_timestamp_nanos: i64) -> f64 {
        let Some(previous) = self.last_timestamp_nanos else {
            self.last_timestamp_nanos = Some(sensor_timestamp_nanos);
            return 0.0;
        };
        // Кадр старше уже учтённого (пришёл не по порядку): шага нет и метку не откатываем,
        // иначе следующий кадр засчитал бы тот же отрезок времени дважды (ревью 08).
        if sensor_timestamp_nanos <= previous {
            return 0.0;
        }
        self.last_timestamp_nanos = Some(sensor_timestamp_nanos);
        // i64 → f64: разность меток кадров — десятки миллисекунд, точность f64 избыточна.
        let seconds = (sensor_timestamp_nanos.saturating_sub(previous)) as f64 / NANOS_PER_SECOND;
        seconds.clamp(0.0, self.config.controller.max_step_seconds)
    }

    /// Ручная точка, прижатая к таблице камеры (прижим виден в `limit`).
    fn manual_target_for(&self, setting: WhiteBalanceSetting) -> ManualTarget {
        let locus = &self.config.locus;
        let requested_kelvin = setting.temperature.kelvin();
        let requested_duv = setting.tint.duv();
        let applied_kelvin = requested_kelvin.clamp(locus.min_temperature_kelvin, locus.max_temperature_kelvin);
        let max_duv = self.config.controller.manual_max_abs_duv;
        let applied_duv = requested_duv.clamp(-max_duv, max_duv);
        let limited = applied_kelvin != requested_kelvin || applied_duv != requested_duv;
        ManualTarget {
            state: WhiteState {
                mired: Kelvin(applied_kelvin).mired(),
                duv: applied_duv,
            },
            limit: limited.then_some(ManualLimit {
                requested_kelvin,
                applied_kelvin,
                requested_duv,
                applied_duv,
            }),
        }
    }
}

/// `(майред, Duv)` → белая цветность.
fn scene_white(state: WhiteState) -> Result<SceneWhite, WhitePointError> {
    let cct_duv = CctDuv {
        temperature: Kelvin(1.0e6 / state.mired),
        duv: Duv(state.duv),
    };
    Ok(SceneWhite {
        chromaticity: chromaticity_from_cct_duv(cct_duv)?,
        cct_duv,
    })
}

#[cfg(test)]
#[path = "controller_tests.rs"]
mod tests;
