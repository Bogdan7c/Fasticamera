//! Компенсация экспозиции («солнце» у рамки фокуса и панель Exposure).
//!
//! Экран — **единственный владелец** значения (решение «один владелец, без эха»): значение хранится
//! в целых шагах, камере уходит эффект `SetExposureBias` только когда его изменил пользователь.
//! Программная установка ([`ExposureState::restore`]) эффекта не порождает.
//!
//! Пределы: из конфига (`±2 EV`, шаг 1/3 EV) **пересекаются** с диапазоном, который сообщила камера.

use camera_contract::control::ExposureBias;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Допуск сравнения долей шага: `0,3333…` не должно давать лишний шаг из-за округления `f32`.
const STEP_EPSILON: f64 = 1e-4;

/// Настройки экспозиции.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureConfig {
    /// Шаг шкалы, EV. `[measured]` 1/3 EV (панель Exposure iOS 27, `behavior.md` 2.1).
    pub step_ev: f32,
    /// Нижний предел, EV. `[estimate]` −2 по симметрии (на видео достигнут только верхний).
    pub min_ev: f32,
    /// Верхний предел, EV. `[measured]` +2 (значение стоит на пределе при движущемся пальце).
    pub max_ev: f32,
}

impl Default for ExposureConfig {
    fn default() -> Self {
        Self {
            step_ev: 1.0 / 3.0,
            min_ev: -2.0,
            max_ev: 2.0,
        }
    }
}

/// Ошибка конфига экспозиции.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum ExposureConfigError {
    #[error("шаг EV должен быть конечным и > 0, получено {0}")]
    InvalidStep(f32),
    #[error("пределы EV должны быть конечными, min ≤ 0 ≤ max: min {min}, max {max}")]
    InvalidRange { min: f32, max: f32 },
}

impl ExposureConfig {
    pub fn validate(&self) -> Result<(), ExposureConfigError> {
        if !(self.step_ev.is_finite() && self.step_ev > 0.0) {
            return Err(ExposureConfigError::InvalidStep(self.step_ev));
        }
        let finite = self.min_ev.is_finite() && self.max_ev.is_finite();
        if !finite || self.min_ev > 0.0 || self.max_ev < 0.0 {
            return Err(ExposureConfigError::InvalidRange {
                min: self.min_ev,
                max: self.max_ev,
            });
        }
        Ok(())
    }
}

/// Что камера сообщила о диапазоне компенсации.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ExposureRangeFact {
    /// Камера ещё не сообщила диапазон: действуют пределы конфига.
    #[default]
    NotReported,
    /// Компенсации нет (диапазон `[0, 0]`): значок и «солнце» ничего не меняют.
    Unsupported,
    /// Диапазон камеры, EV.
    Supported { min_ev: f32, max_ev: f32 },
}

/// Пределы в целых шагах (включительно). Всегда `min_steps ≤ 0 ≤ max_steps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureLimits {
    min_steps: i32,
    max_steps: i32,
}

impl ExposureLimits {
    /// Пересечение пределов конфига с диапазоном камеры, округлённое внутрь до целых шагов.
    pub fn resolve(config: &ExposureConfig, camera: ExposureRangeFact) -> Self {
        let (camera_min, camera_max) = match camera {
            ExposureRangeFact::NotReported => (f32::NEG_INFINITY, f32::INFINITY),
            ExposureRangeFact::Unsupported => (0.0, 0.0),
            ExposureRangeFact::Supported { min_ev, max_ev } => (min_ev, max_ev),
        };
        let step = f64::from(config.step_ev);
        let min_ev = f64::from(config.min_ev.max(camera_min));
        let max_ev = f64::from(config.max_ev.min(camera_max));
        // Внутрь: нижняя граница округляется вверх, верхняя — вниз; 0 всегда допустим.
        let min_steps = ((min_ev / step) - STEP_EPSILON).ceil().min(0.0) as i32;
        let max_steps = ((max_ev / step) + STEP_EPSILON).floor().max(0.0) as i32;
        Self { min_steps, max_steps }
    }

    /// Можно ли вообще менять значение.
    pub fn is_adjustable(self) -> bool {
        self.min_steps < self.max_steps
    }

    pub fn min_steps(self) -> i32 {
        self.min_steps
    }

    pub fn max_steps(self) -> i32 {
        self.max_steps
    }

    fn clamp(self, steps: i32) -> i32 {
        steps.clamp(self.min_steps, self.max_steps)
    }
}

/// Итог пользовательской установки значения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExposureChange {
    /// Значение стало другим: камере нужно сообщить новое.
    Changed,
    /// Значение уже было таким: ничего делать не надо.
    Unchanged,
}

/// Текущая компенсация экспозиции экрана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureState {
    steps: i32,
    limits: ExposureLimits,
}

impl ExposureState {
    /// Начальное состояние: ноль, пределы конфига.
    pub fn new(config: &ExposureConfig) -> Self {
        Self {
            steps: 0,
            limits: ExposureLimits::resolve(config, ExposureRangeFact::NotReported),
        }
    }

    /// Значение в шагах (для шкалы).
    pub fn steps(&self) -> i32 {
        self.steps
    }

    /// Значение в EV для подписи («+0.3 EV») и для камеры.
    pub fn bias(&self, config: &ExposureConfig) -> ExposureBias {
        // from_ev отвергает только NaN/∞, а шаг и число шагов конечны — запасной ноль недостижим.
        ExposureBias::from_ev(self.steps as f32 * config.step_ev).unwrap_or(ExposureBias::NONE)
    }

    pub fn limits(&self) -> ExposureLimits {
        self.limits
    }

    /// Квантует запрошенное значение к шагу и пределам.
    pub fn quantize(&self, requested: ExposureBias, config: &ExposureConfig) -> i32 {
        let raw_steps = (f64::from(requested.ev()) / f64::from(config.step_ev)).round();
        // Сужение f64 → i32 безопасно: значение сначала зажимается в пределах i32 через пределы шкалы.
        let bounded = raw_steps.clamp(f64::from(self.limits.min_steps), f64::from(self.limits.max_steps));
        bounded as i32
    }

    /// Пользователь выбрал значение; итог говорит, изменилось ли оно.
    pub(crate) fn set_by_user(&mut self, steps: i32) -> ExposureChange {
        let change = if self.steps == steps {
            ExposureChange::Unchanged
        } else {
            ExposureChange::Changed
        };
        self.steps = steps;
        change
    }

    /// Программная установка (восстановление настроек): квантуется и зажимается, эффекта нет.
    pub(crate) fn restore(&mut self, requested: ExposureBias, config: &ExposureConfig) {
        self.steps = self.quantize(requested, config);
    }

    /// Камера сообщила новый диапазон. Возвращает `true`, если текущее значение пришлось сдвинуть
    /// (тогда камере надо сообщить новое значение — иначе экран и камера разойдутся).
    pub(crate) fn apply_camera_range(&mut self, config: &ExposureConfig, camera: ExposureRangeFact) -> bool {
        self.limits = ExposureLimits::resolve(config, camera);
        let clamped = self.limits.clamp(self.steps);
        let moved = clamped != self.steps;
        self.steps = clamped;
        moved
    }
}
