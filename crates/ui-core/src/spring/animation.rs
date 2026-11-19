//! Скалярная пружинная анимация с прерыванием по правилу «замена» (SwiftUI `spring`).

use super::model::{Spring, SpringError};
use super::rest::{RestCriterion, RestTolerance};
use super::solution::SpringState;
use crate::time::{MonotonicNanos, UiDuration};

/// Фаза анимации в момент выборки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpringPhase {
    Moving,
    /// Покой: значение равно цели точно, скорость нулевая.
    AtRest,
}

/// Результат выборки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringSample {
    pub value: f64,
    /// Скорость, единицы величины в секунду (pt/с).
    pub velocity: f64,
    pub phase: SpringPhase,
}

/// Параметры старта анимации (именованные поля вместо шести позиционных аргументов).
#[derive(Debug, Clone, Copy)]
pub struct SpringStart {
    pub spring: Spring,
    pub from: f64,
    pub to: f64,
    /// Начальная скорость, pt/с (скорость пальца при отпускании).
    pub initial_velocity: f64,
    pub start_time: MonotonicNanos,
    pub rest: RestCriterion,
}

/// Анимация одной величины к цели. Считается аналитически от `now − start_time`.
///
/// Время до старта не экстраполируется: выборка до `start_time` равна выборке в `start_time`.
#[derive(Debug, Clone, Copy)]
pub struct SpringAnimation {
    spring: Spring,
    target: f64,
    initial: SpringState,
    start_time: MonotonicNanos,
    rest: RestCriterion,
    tolerance: RestTolerance,
}

impl SpringAnimation {
    /// Начать анимацию. Ошибка при нефинитных входах или некорректном критерии покоя.
    pub fn start(params: SpringStart) -> Result<Self, SpringError> {
        params.rest.validate()?;
        ensure_finite("from", params.from)?;
        ensure_finite("to", params.to)?;
        ensure_finite("initial_velocity", params.initial_velocity)?;
        Ok(Self::build(
            params.spring,
            params.to,
            SpringState {
                displacement: params.from - params.to,
                velocity: params.initial_velocity,
            },
            params.start_time,
            params.rest,
        ))
    }

    fn build(
        spring: Spring,
        target: f64,
        initial: SpringState,
        start_time: MonotonicNanos,
        rest: RestCriterion,
    ) -> Self {
        // У `immediate` покой наступает всегда (см. `sample`), частота допуску не нужна.
        let frequency = spring.angular_frequency().unwrap_or(0.0);
        let tolerance = rest.tolerance_for_distance(initial.displacement, frequency);
        Self {
            spring,
            target,
            initial,
            start_time,
            rest,
            tolerance,
        }
    }

    /// Текущая цель.
    pub fn target(&self) -> f64 {
        self.target
    }

    /// Пружина, ведущая анимацию сейчас.
    pub fn spring(&self) -> Spring {
        self.spring
    }

    /// Момент начала текущего отрезка (после `retarget` — момент перенацеливания).
    pub fn start_time(&self) -> MonotonicNanos {
        self.start_time
    }

    /// Значение, скорость и фаза в момент `now`.
    pub fn sample(&self, now: MonotonicNanos) -> SpringSample {
        let elapsed_s = UiDuration::between(self.start_time, now).as_secs_f64().max(0.0);
        let state = self.spring.evolve(self.initial, elapsed_s);
        if self.spring.is_immediate() || self.tolerance.contains(state.displacement, state.velocity) {
            return SpringSample {
                value: self.target,
                velocity: 0.0,
                phase: SpringPhase::AtRest,
            };
        }
        SpringSample {
            value: self.target + state.displacement,
            velocity: state.velocity,
            phase: SpringPhase::Moving,
        }
    }

    /// Новая цель той же пружиной. Режим «замена»: новая пружина стартует от ТЕКУЩИХ положения
    /// и скорости (`motion.md` 1.4); скорость наследуется, скачка нет.
    pub fn retarget(&mut self, new_target: f64, now: MonotonicNanos) -> Result<(), SpringError> {
        self.retarget_with(self.spring, new_target, now)
    }

    /// То же, но со сменой пружины на лету.
    ///
    /// Если `now` раньше старта, перенацеливание считается от старта (анимация ещё не началась).
    pub fn retarget_with(&mut self, spring: Spring, new_target: f64, now: MonotonicNanos) -> Result<(), SpringError> {
        ensure_finite("new_target", new_target)?;
        let effective_now = now.max(self.start_time);
        let current = self.sample(effective_now);
        *self = Self::build(
            spring,
            new_target,
            SpringState {
                displacement: current.value - new_target,
                velocity: current.velocity,
            },
            effective_now,
            self.rest,
        );
        Ok(())
    }
}

pub(super) fn ensure_finite(field: &'static str, value: f64) -> Result<(), SpringError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(SpringError::NonFiniteInput { field, value })
    }
}

/// Скорость UIKit-стиля «доли расстояния в секунду» → pt/с: `v = доля · (to − from)`
/// (`motion.md` 1.4, `UISpringTimingParameters.initialVelocity`).
pub fn velocity_from_distance_fraction(fraction_per_second: f64, from: f64, to: f64) -> f64 {
    fraction_per_second * (to - from)
}

/// pt/с → доли расстояния в секунду; при нулевом расстоянии 0 (правило UIKit).
pub fn distance_fraction_per_second(velocity: f64, from: f64, to: f64) -> f64 {
    let distance = to - from;
    if distance == 0.0 { 0.0 } else { velocity / distance }
}
