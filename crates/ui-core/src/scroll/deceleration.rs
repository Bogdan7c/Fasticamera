//! Замедление брошенного контента (`motion.md` раздел 2).
//!
//! Модель одна и для траектории, и для проекции: непрерывное экспоненциальное затухание
//! `v(t) = v0·e^(−λt)`, `λ = −1000·ln d` (`d` — доля скорости за 1 мс). Тогда бросок приезжает
//! ровно туда, куда показала проекция. Формула WWDC `(v0/1000)·d/(1−d)` — дискретная сумма
//! по миллисекундам; она отличается от интеграла `v0/λ` на ~0,1 % при d = 0,998 и ~0,5 % при
//! d = 0,99 (см. [`DecelerationRate::wwdc_discrete_projection`]).

use serde::{Deserialize, Serialize};

use super::error::ScrollError;
use crate::time::{MonotonicNanos, UiDuration};

/// Миллисекунд в секунде: `d` задана «за 1 мс».
const MILLIS_PER_SECOND: f64 = 1000.0;

/// Доля скорости, остающаяся за 1 мс. Допустимо только `0 < d < 1`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct DecelerationRate(f64);

impl DecelerationRate {
    /// `.normal`: 0,998. `[review]` [teletype](https://teletype.in/@swift-discipline/uiscrollview-deceleration);
    /// в текущей документации Apple числа нет.
    pub const NORMAL_PER_MILLISECOND: f64 = 0.998;
    /// `.fast`: 0,99. `[review]` там же.
    pub const FAST_PER_MILLISECOND: f64 = 0.99;

    pub fn new(per_millisecond: f64) -> Result<Self, ScrollError> {
        if per_millisecond.is_finite() && per_millisecond > 0.0 && per_millisecond < 1.0 {
            Ok(Self(per_millisecond))
        } else {
            Err(ScrollError::InvalidDecelerationRate(per_millisecond))
        }
    }

    pub const fn normal() -> Self {
        Self(Self::NORMAL_PER_MILLISECOND)
    }

    pub const fn fast() -> Self {
        Self(Self::FAST_PER_MILLISECOND)
    }

    pub fn per_millisecond(&self) -> f64 {
        self.0
    }

    /// Показатель затухания `λ`, 1/с: `v(t) = v0·e^(−λt)`.
    pub fn decay_per_second(&self) -> f64 {
        -MILLIS_PER_SECOND * self.0.ln()
    }

    /// Формула со слайда WWDC18 803, `[review]` ([gist](https://gist.github.com/shaps80/641b61165fd5f8bd497e40f4d16d69be)):
    /// `(v0/1000)·d/(1−d)`. Только для сверки: движок использует непрерывную модель.
    pub fn wwdc_discrete_projection(&self, velocity: f64) -> f64 {
        velocity / MILLIS_PER_SECOND * self.0 / (1.0 - self.0)
    }
}

impl TryFrom<f64> for DecelerationRate {
    type Error = ScrollError;
    fn try_from(per_millisecond: f64) -> Result<Self, ScrollError> {
        Self::new(per_millisecond)
    }
}

impl From<DecelerationRate> for f64 {
    fn from(rate: DecelerationRate) -> f64 {
        rate.0
    }
}

/// Замедление: доля за 1 мс и порог, ниже которого контент считается остановившимся.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecelerationModel {
    rate: DecelerationRate,
    stop_velocity: f64,
}

impl DecelerationModel {
    /// `stop_velocity` — модуль скорости (pt/с), ниже которого движение прекращается.
    pub fn new(rate: DecelerationRate, stop_velocity: f64) -> Result<Self, ScrollError> {
        if !stop_velocity.is_finite() || stop_velocity <= 0.0 {
            return Err(ScrollError::InvalidStopVelocity(stop_velocity));
        }
        Ok(Self { rate, stop_velocity })
    }

    pub fn rate(&self) -> DecelerationRate {
        self.rate
    }

    pub fn stop_velocity(&self) -> f64 {
        self.stop_velocity
    }

    /// Куда (на сколько pt, со знаком) уедет бросок со скоростью `velocity` до остановки.
    /// Это и есть проекция для прилипания: траектория [`DecelerationSegment`] кончается ровно тут.
    pub fn stopping_distance(&self, velocity: f64) -> f64 {
        let speed = velocity.abs();
        if speed <= self.stop_velocity {
            return 0.0;
        }
        ((speed - self.stop_velocity) / self.rate.decay_per_second()).copysign(velocity)
    }
}

/// Чем кончается отрезок замедления.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DecelerationEnd {
    /// Остановился внутри границ.
    Stops,
    /// Дошёл до границы со скоростью `speed` (модуль) и должен перейти в возврат пружиной.
    HitsEdge { edge_offset: f64, speed: f64 },
}

/// Запланированный бросок от `from` со скоростью `velocity` в границах `[min, max]`.
/// Всё считается заранее и аналитически (по времени от старта): сэмплирование чистое.
#[derive(Debug, Clone, Copy)]
pub struct DecelerationSegment {
    start_time: MonotonicNanos,
    from: f64,
    direction: f64,
    initial_speed: f64,
    decay_per_second: f64,
    end_after_s: f64,
    end_offset: f64,
    end: DecelerationEnd,
}

impl DecelerationSegment {
    /// Спланировать бросок. `from` обязан лежать в границах (за краем нужен возврат, не бросок).
    pub fn plan(
        model: DecelerationModel,
        bounds: (f64, f64),
        start_time: MonotonicNanos,
        from: f64,
        velocity: f64,
    ) -> Self {
        let decay = model.rate.decay_per_second();
        let direction = if velocity < 0.0 { -1.0 } else { 1.0 };
        let initial_speed = velocity.abs();
        let stopped = Self {
            start_time,
            from,
            direction,
            initial_speed,
            decay_per_second: decay,
            end_after_s: 0.0,
            end_offset: from,
            end: DecelerationEnd::Stops,
        };
        if initial_speed <= model.stop_velocity {
            return stopped;
        }
        let distance_to_edge = if direction > 0.0 {
            bounds.1 - from
        } else {
            from - bounds.0
        }
        .max(0.0);
        let total_distance = (initial_speed - model.stop_velocity) / decay;
        if total_distance < distance_to_edge {
            return Self {
                end_after_s: (initial_speed / model.stop_velocity).ln() / decay,
                end_offset: from + direction * total_distance,
                ..stopped
            };
        }
        // Скорость у края из `v(t_edge) = v0 − λ·расстояние`; она выше порога, раз бросок «длиннее» края.
        let edge_speed = initial_speed - decay * distance_to_edge;
        let edge_offset = from + direction * distance_to_edge;
        Self {
            end_after_s: (initial_speed / edge_speed).ln() / decay,
            end_offset: edge_offset,
            end: DecelerationEnd::HitsEdge {
                edge_offset,
                speed: edge_speed,
            },
            ..stopped
        }
    }

    pub fn end(&self) -> DecelerationEnd {
        self.end
    }

    /// Положение, где отрезок заканчивается (останов или край).
    pub fn end_offset(&self) -> f64 {
        self.end_offset
    }

    /// Момент окончания отрезка.
    pub fn end_time(&self) -> MonotonicNanos {
        crate::time::add_duration(self.start_time, UiDuration::from_secs_f64(self.end_after_s))
    }

    /// Скорость (со знаком) в момент выхода на край; для остановки — 0.
    pub fn end_velocity(&self) -> f64 {
        match self.end {
            DecelerationEnd::Stops => 0.0,
            DecelerationEnd::HitsEdge { speed, .. } => speed.copysign(self.direction),
        }
    }

    /// Положение, скорость и признак «отрезок закончился» в момент `now`.
    pub fn sample(&self, now: MonotonicNanos) -> (f64, f64, bool) {
        let elapsed_s = UiDuration::between(self.start_time, now).as_secs_f64().max(0.0);
        if elapsed_s >= self.end_after_s {
            return (self.end_offset, self.end_velocity(), true);
        }
        let speed = self.initial_speed * (-self.decay_per_second * elapsed_s).exp();
        let offset = self.from + self.direction * (self.initial_speed - speed) / self.decay_per_second;
        (offset, speed.copysign(self.direction), false)
    }
}
