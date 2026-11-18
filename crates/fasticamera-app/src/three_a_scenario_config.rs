//! Секция `camera.scenarios.three_a`: параметры smoke-сценария `3a` (сессия 08).
//!
//! Режим камеры, сроки запуска и роль берутся из `camera.scenarios.viewfinder` (сценарий `3a` стартует
//! продуктовый видоискатель тем же запуском); здесь только то, что относится к проверкам автоматики:
//! сроки каждого шага, точки тапа, ручные значения, допуски и пороги. Каждый допуск имеет источник в
//! комментарии: число без причины читать нечем.

use serde::{Deserialize, Serialize};

/// Одна секунда в миллисекундах — для читаемости значений по умолчанию.
const SECOND_MS: u64 = 1_000;
/// Нс в мс: выдержки в конфиге задаются в наносекундах (так их отдаёт HAL, зонд 08: 996 220 нс).
const NS_PER_MS: u64 = 1_000_000;

/// Точка тапа в нормированных координатах сенсора `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapPointConfig {
    pub x: f32,
    pub y: f32,
}

/// Шаг AWB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AwbStepConfig {
    /// Сколько ждать первой оценки света и конца холодного старта, мс.
    pub ready_timeout_ms: u64,
    /// Выше этой разницы нашей белой и белой HAL (майреды) в отчёте стоит пометка `awb_vs_hal: warn`. Не провал:
    /// белая HAL — только справка (решение 62). 25 майред ≈ 300 К на 5000 К — заметный сдвиг цвета.
    pub warn_mired_vs_hal: f64,
}

/// Шаг тапа фокуса.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TapStepConfig {
    /// Точки по порядку. Первая — центр: по ней судится правило «тап сходится». Остальные — для вопроса
    /// о регионах (разные ли дистанции у разных точек кадра), его судит владелец по сцене с ближним и дальним.
    pub points: Vec<TapPointConfig>,
    /// Сколько ждать итога тапа, мс. Больше `three_a.focus_settle_timeout_ms` движка, чтобы итог `TimedOut`
    /// успел прийти событием.
    pub settle_timeout_ms: u64,
}

/// Шаг ручной экспозиции.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ManualStepConfig {
    /// ISO ручного режима.
    pub iso: u32,
    /// Выдержка ручного режима, нс (1/1000 с).
    pub shutter_ns: u64,
    /// Нарочно слишком длинная выдержка, нс (1/10 с при 60 к/с не помещается в кадр).
    pub long_shutter_ns: u64,
    /// Допуск выдержки и ISO в результате, доля. Зонд видел 996 220 нс при просьбе 1 000 000: 0,4 %; 1 % — запас.
    pub tolerance_fraction: f64,
    /// Сколько ждать, пока результат покажет новую экспозицию, мс.
    pub apply_timeout_ms: u64,
}

/// Шаг компенсации экспозиции.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EvStepConfig {
    /// Компенсации по порядку; первая — 0 (опорная).
    pub biases_ev: Vec<f32>,
    /// Пауза на сходимость AE после смены компенсации, мс.
    pub settle_ms: u64,
    /// Сколько разных кадров брать на медиану.
    pub samples: usize,
    /// Сколько ждать набора кадров, мс.
    pub collect_timeout_ms: u64,
    /// Допуск отношения «экспозиция × ISO» к опорному относительно 2^EV, доля. Вендорский AE квантует
    /// компенсацию шагом 1/6 EV и сходится к цели не точно: 0,35 ловит «компенсация не действует»
    /// (отношение 1,0 вместо 2,0), но не шум сходимости.
    pub ratio_tolerance_fraction: f64,
}

/// Шаг блокировки AE/AF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LockStepConfig {
    /// Сколько ждать `AE_STATE = Locked` после блокировки, мс.
    pub settle_timeout_ms: u64,
    /// Сколько держать блокировку под наблюдением, мс.
    pub hold_ms: u64,
    /// Допуск постоянства «экспозиция × ISO» под блокировкой, доля.
    pub constancy_tolerance_fraction: f64,
    /// Сколько ждать возврата непрерывного AF после сброса точки, мс.
    pub unlock_timeout_ms: u64,
    /// Наш AE: сколько кадров ждать после `Holding`, пока замороженная цель дойдёт до сенсора (HAL видит
    /// новую экспозицию через 7-8 кадров), прежде чем судить постоянство «выдержка × ISO». Вдвое больше
    /// задержки зонда 08 — оценка, сверить в 08c. Для вендорского AE не используется.
    pub own_ae_catch_up_frames: u32,
}

/// Шаг брекетинга.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BracketStepConfig {
    /// Выдержки кадров серии, нс.
    pub exposures_ns: Vec<u64>,
    pub iso: u32,
    /// Предел длины серии для конструктора `ExposureBracket` (тот же, что `camera.engine.three_a.max_bracket_frames`).
    pub frame_limit: usize,
    /// Сколько ждать итога серии, мс.
    pub result_timeout_ms: u64,
    /// Допуск выдержки и ISO кадра серии, доля. Короткие выдержки квантуются строкой сенсора: 2 %.
    pub tolerance_fraction: f64,
    /// Пауза после итога серии, чтобы рендер успел принять и пропустить её кадры, мс.
    pub render_settle_ms: u64,
    /// На сколько меньше длины серии может оказаться прирост `bracket_frames_skipped` (кадр серии, вытесненный
    /// из очереди видоискателя до опроса). Больше длины серии прироста быть не может никогда.
    pub max_skipped_shortfall: u64,
    /// Сколько потерь кадров видоискателя сверх обычных допустимо за время серии (вытеснение, переполнение буфера).
    pub max_extra_dropped_frames: u64,
}

/// Секция `camera.scenarios.three_a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ThreeAScenarioConfig {
    /// Сколько ждать первых результатов с `AF_STATE` и `AE_STATE` после запуска видоискателя, мс.
    pub live_results_timeout_ms: u64,
    pub awb: AwbStepConfig,
    pub tap: TapStepConfig,
    pub manual: ManualStepConfig,
    pub ev: EvStepConfig,
    pub lock: LockStepConfig,
    pub bracket: BracketStepConfig,
}

impl Default for ThreeAScenarioConfig {
    fn default() -> Self {
        Self {
            live_results_timeout_ms: 3 * SECOND_MS,
            awb: AwbStepConfig::default(),
            tap: TapStepConfig::default(),
            manual: ManualStepConfig::default(),
            ev: EvStepConfig::default(),
            lock: LockStepConfig::default(),
            bracket: BracketStepConfig::default(),
        }
    }
}

impl Default for AwbStepConfig {
    fn default() -> Self {
        Self {
            ready_timeout_ms: 5 * SECOND_MS,
            warn_mired_vs_hal: 25.0,
        }
    }
}

impl Default for TapStepConfig {
    fn default() -> Self {
        let point = |x, y| TapPointConfig { x, y };
        Self {
            // Центр и четыре точки у углов (на 15 % от краёв: рамка региона целиком в кадре).
            points: vec![
                point(0.5, 0.5),
                point(0.15, 0.15),
                point(0.85, 0.15),
                point(0.15, 0.85),
                point(0.85, 0.85),
            ],
            settle_timeout_ms: 6 * SECOND_MS,
        }
    }
}

impl Default for ManualStepConfig {
    fn default() -> Self {
        Self {
            iso: 400,
            shutter_ns: NS_PER_MS,
            long_shutter_ns: 100 * NS_PER_MS,
            tolerance_fraction: 0.01,
            apply_timeout_ms: 3 * SECOND_MS,
        }
    }
}

impl Default for EvStepConfig {
    fn default() -> Self {
        Self {
            biases_ev: vec![0.0, 1.0, -1.0],
            settle_ms: 1_500,
            samples: 20,
            collect_timeout_ms: 4 * SECOND_MS,
            ratio_tolerance_fraction: 0.35,
        }
    }
}

impl Default for LockStepConfig {
    fn default() -> Self {
        Self {
            settle_timeout_ms: 5 * SECOND_MS,
            hold_ms: 2 * SECOND_MS,
            constancy_tolerance_fraction: 0.01,
            unlock_timeout_ms: 3 * SECOND_MS,
            own_ae_catch_up_frames: 15,
        }
    }
}

impl Default for BracketStepConfig {
    fn default() -> Self {
        Self {
            exposures_ns: vec![2 * NS_PER_MS, 4 * NS_PER_MS, 8 * NS_PER_MS, 16 * NS_PER_MS],
            iso: 400,
            frame_limit: 4,
            result_timeout_ms: 5 * SECOND_MS,
            tolerance_fraction: 0.02,
            render_settle_ms: 300,
            max_skipped_shortfall: 0,
            max_extra_dropped_frames: 2,
        }
    }
}

/// Неверная секция `camera.scenarios.three_a`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ThreeAScenarioConfigError {
    #[error("three_a.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("three_a.{field} = {value}: нужна доля больше 0 и меньше 1")]
    FractionOutOfRange { field: &'static str, value: f64 },
    #[error("three_a.awb.warn_mired_vs_hal = {value}: нужно конечное положительное число")]
    BadMiredThreshold { value: f64 },
    #[error("three_a.tap.points: точек нет — тапать некуда")]
    NoTapPoints,
    #[error("three_a.tap.points[{index}] = ({x}, {y}): координаты должны быть в [0, 1]")]
    TapPointOutOfFrame { index: usize, x: f32, y: f32 },
    #[error("three_a.tap.points[0] должна быть центром (0.5, 0.5): по ней судится правило сходимости")]
    FirstTapPointNotCenter,
    #[error("three_a.ev.biases_ev: первая компенсация должна быть 0 (опорная), остальные конечные и разные")]
    BadEvBiases,
    #[error("three_a.bracket.exposures_ns: пусто или длиннее frame_limit ({limit})")]
    BadBracketLength { limit: usize },
    #[error("three_a.manual.long_shutter_ns должна быть больше shutter_ns")]
    LongShutterNotLonger,
}

impl ThreeAScenarioConfig {
    pub fn validate(&self) -> Result<(), ThreeAScenarioConfigError> {
        for (field, value) in [
            ("live_results_timeout_ms", self.live_results_timeout_ms),
            ("awb.ready_timeout_ms", self.awb.ready_timeout_ms),
            ("tap.settle_timeout_ms", self.tap.settle_timeout_ms),
            ("manual.apply_timeout_ms", self.manual.apply_timeout_ms),
            ("manual.iso", u64::from(self.manual.iso)),
            ("manual.shutter_ns", self.manual.shutter_ns),
            ("ev.settle_ms", self.ev.settle_ms),
            ("ev.samples", self.ev.samples as u64),
            ("ev.collect_timeout_ms", self.ev.collect_timeout_ms),
            ("lock.settle_timeout_ms", self.lock.settle_timeout_ms),
            ("lock.hold_ms", self.lock.hold_ms),
            ("lock.unlock_timeout_ms", self.lock.unlock_timeout_ms),
            ("bracket.iso", u64::from(self.bracket.iso)),
            ("bracket.result_timeout_ms", self.bracket.result_timeout_ms),
            ("bracket.frame_limit", self.bracket.frame_limit as u64),
        ] {
            if value == 0 {
                return Err(ThreeAScenarioConfigError::Zero { field });
            }
        }
        for (field, value) in [
            ("manual.tolerance_fraction", self.manual.tolerance_fraction),
            ("ev.ratio_tolerance_fraction", self.ev.ratio_tolerance_fraction),
            (
                "lock.constancy_tolerance_fraction",
                self.lock.constancy_tolerance_fraction,
            ),
            ("bracket.tolerance_fraction", self.bracket.tolerance_fraction),
        ] {
            // Через сравнение: NaN не проходит.
            if !(value > 0.0 && value < 1.0) {
                return Err(ThreeAScenarioConfigError::FractionOutOfRange { field, value });
            }
        }
        let mired = self.awb.warn_mired_vs_hal;
        if !(mired.is_finite() && mired > 0.0) {
            return Err(ThreeAScenarioConfigError::BadMiredThreshold { value: mired });
        }
        self.validate_tap_points()?;
        self.validate_ev_and_bracket()?;
        if self.manual.long_shutter_ns <= self.manual.shutter_ns {
            return Err(ThreeAScenarioConfigError::LongShutterNotLonger);
        }
        Ok(())
    }

    fn validate_tap_points(&self) -> Result<(), ThreeAScenarioConfigError> {
        let points = &self.tap.points;
        let first = points.first().ok_or(ThreeAScenarioConfigError::NoTapPoints)?;
        for (index, point) in points.iter().enumerate() {
            let inside = (0.0..=1.0).contains(&point.x) && (0.0..=1.0).contains(&point.y);
            if !inside {
                return Err(ThreeAScenarioConfigError::TapPointOutOfFrame {
                    index,
                    x: point.x,
                    y: point.y,
                });
            }
        }
        if (first.x, first.y) != (0.5, 0.5) {
            return Err(ThreeAScenarioConfigError::FirstTapPointNotCenter);
        }
        Ok(())
    }

    fn validate_ev_and_bracket(&self) -> Result<(), ThreeAScenarioConfigError> {
        let biases = &self.ev.biases_ev;
        let distinct = biases
            .iter()
            .enumerate()
            .all(|(index, bias)| !biases[..index].contains(bias));
        if biases.first() != Some(&0.0) || !distinct || biases.iter().any(|bias| !bias.is_finite()) {
            return Err(ThreeAScenarioConfigError::BadEvBiases);
        }
        let length = self.bracket.exposures_ns.len();
        if length == 0 || length > self.bracket.frame_limit || self.bracket.exposures_ns.contains(&0) {
            return Err(ThreeAScenarioConfigError::BadBracketLength {
                limit: self.bracket.frame_limit,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "three_a_scenario_config_tests.rs"]
mod tests;
