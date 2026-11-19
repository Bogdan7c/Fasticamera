//! Пружинная величина «прогресс 0..1» — общая основа K7–K10 (панель, плашка, рамка, затвор).
//!
//! Своей физики здесь нет: это тонкая обёртка над [`SpringAnimation`] из `ui-core` с тремя
//! правилами, одинаковыми для всех элементов (правило «без дублирования», решение 84):
//! * время приходит снаружи, значение считается от «сейчас − старт», поэтому выборки на 60 и
//!   120 Гц в общих моментах совпадают;
//! * перенацеливание (закрыли во время открытия) наследует положение **и скорость** — скачка нет;
//! * покой определяется допуском, подходящим для единичной шкалы ([`UNIT_REST`]), а не для pt.

use ui_core::spring::{RestCriterion, Spring, SpringAnimation, SpringError, SpringPhase, SpringStart};
use ui_core::time::{MonotonicNanos, MotionPreference};

use crate::tokens::{MotionToken, MotionTokens, TokenError};

/// Критерий покоя для величин порядка единицы (прогресс 0..1, масштаб, прозрачность).
///
/// Умолчание `ui-core` рассчитано на pt (пол допуска 0,01 — одна сотая пикселя) и для доли
/// единицы было бы слишком грубым: рамка панели в 300 pt «прыгнула» бы на 3 pt при остановке.
/// `[estimate]` — подгонка, не Apple: пол 0,0001 даёт скачок <0,03 pt на 300 pt.
pub(crate) const UNIT_REST: RestCriterion = RestCriterion {
    relative_epsilon: 0.001,
    absolute_position_floor: 0.0001,
    velocity_tolerance_factor: 1.0,
};

/// Скалярная величина, едущая к цели пружиной.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpringProgress {
    animation: SpringAnimation,
}

impl SpringProgress {
    /// Величина в покое на `value`.
    pub(crate) fn at_rest(value: f64, spring: Spring, now: MonotonicNanos) -> Result<Self, SpringError> {
        Self::starting(spring, value, value, now)
    }

    /// Величина, которая стартует в `from` и едет к `to` с нулевой скоростью.
    pub(crate) fn starting(spring: Spring, from: f64, to: f64, now: MonotonicNanos) -> Result<Self, SpringError> {
        Ok(Self {
            animation: SpringAnimation::start(SpringStart {
                spring,
                from,
                to,
                initial_velocity: 0.0,
                start_time: now,
                rest: UNIT_REST,
            })?,
        })
    }

    pub(crate) fn value(&self, now: MonotonicNanos) -> f64 {
        self.animation.sample(now).value
    }

    /// Величина ещё едет (не пришла к цели в пределах допуска).
    pub(crate) fn is_moving(&self, now: MonotonicNanos) -> bool {
        self.animation.sample(now).phase == SpringPhase::Moving
    }

    pub(crate) fn target(&self) -> f64 {
        self.animation.target()
    }

    /// Новая цель, новая пружина; положение и скорость на момент `now` наследуются.
    pub(crate) fn retarget(&mut self, spring: Spring, target: f64, now: MonotonicNanos) -> Result<(), SpringError> {
        self.animation.retarget_with(spring, target, now)
    }
}

/// Источник пружин элемента: токены движения + предпочтение движения (Reduce Motion).
///
/// Элемент хранит копию токенов движения, а не ссылку: после создания окружение ему не нужно,
/// а Reduce Motion можно переключить на лету ([`SpringSource::set_motion`]).
#[derive(Debug, Clone)]
pub(crate) struct SpringSource {
    tokens: MotionTokens,
    motion: MotionPreference,
}

impl SpringSource {
    pub(crate) fn new(tokens: &MotionTokens, motion: MotionPreference) -> Self {
        Self {
            tokens: tokens.clone(),
            motion,
        }
    }

    /// Готовая пружина по имени токена; Reduce Motion уже применён (правило живёт в токенах).
    pub(crate) fn spring(&self, token: MotionToken) -> Result<Spring, TokenError> {
        self.tokens.spring(token, self.motion)
    }

    pub(crate) fn set_motion(&mut self, motion: MotionPreference) {
        self.motion = motion;
    }
}
