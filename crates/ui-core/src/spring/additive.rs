//! Прерывание по правилу «сложение» (SwiftUI `interpolatingSpring`, `motion.md` 1.4).
//!
//! Значение = цель + сумма смещений независимых вкладов. Каждое перенацеливание добавляет вклад
//! `старая цель − новая цель` с нулевой скоростью; уже летящие вклады продолжают двигаться, и их
//! скорости суммируются. Значение и скорость непрерывны в момент перенацеливания.

use super::animation::{SpringPhase, SpringSample, ensure_finite};
use super::model::{Spring, SpringError};
use super::rest::{RestCriterion, RestTolerance};
use super::solution::SpringState;
use crate::time::{MonotonicNanos, UiDuration};

/// Сколько вкладов живёт одновременно по умолчанию (`[estimate]` — подгонка, не Apple).
pub const DEFAULT_MAX_LIVE_CONTRIBUTIONS: usize = 8;

/// Параметры старта аддитивной анимации.
#[derive(Debug, Clone, Copy)]
pub struct AdditiveSpringStart {
    pub spring: Spring,
    pub from: f64,
    pub to: f64,
    pub initial_velocity: f64,
    pub start_time: MonotonicNanos,
    pub rest: RestCriterion,
    /// Предел живых вкладов (>= 1); при превышении два старейших сливаются.
    pub max_live_contributions: usize,
}

/// Один вклад: своя пружина и свой отсчёт времени.
#[derive(Debug, Clone, Copy)]
struct Contribution {
    spring: Spring,
    initial: SpringState,
    start_time: MonotonicNanos,
    tolerance: RestTolerance,
}

impl Contribution {
    fn state_at(&self, now: MonotonicNanos) -> SpringState {
        let elapsed_s = UiDuration::between(self.start_time, now).as_secs_f64().max(0.0);
        self.spring.evolve(self.initial, elapsed_s)
    }

    /// Живой вклад (не успокоился). Для `immediate` вкладов не заводим вовсе.
    fn live_state_at(&self, now: MonotonicNanos) -> Option<SpringState> {
        let state = self.state_at(now);
        if self.tolerance.contains(state.displacement, state.velocity) {
            None
        } else {
            Some(state)
        }
    }
}

/// Анимация со сложением вкладов.
#[derive(Debug, Clone)]
pub struct AdditiveSpringAnimation {
    target: f64,
    contributions: Vec<Contribution>,
    rest: RestCriterion,
    max_live_contributions: usize,
    last_event_time: MonotonicNanos,
}

impl AdditiveSpringAnimation {
    pub fn start(params: AdditiveSpringStart) -> Result<Self, SpringError> {
        params.rest.validate()?;
        if params.max_live_contributions == 0 {
            return Err(SpringError::InvalidContributionLimit);
        }
        ensure_finite("from", params.from)?;
        ensure_finite("to", params.to)?;
        ensure_finite("initial_velocity", params.initial_velocity)?;
        let mut animation = Self {
            target: params.to,
            contributions: Vec::new(),
            rest: params.rest,
            max_live_contributions: params.max_live_contributions,
            last_event_time: params.start_time,
        };
        animation.push_contribution(
            params.spring,
            SpringState {
                displacement: params.from - params.to,
                velocity: params.initial_velocity,
            },
            params.start_time,
        );
        Ok(animation)
    }

    pub fn target(&self) -> f64 {
        self.target
    }

    /// Сколько вкладов хранится сейчас (включая ещё не вычищенные успокоившиеся).
    pub fn contribution_count(&self) -> usize {
        self.contributions.len()
    }

    /// Выборка: значение = цель + сумма живых вкладов; в покое — ровно цель.
    pub fn sample(&self, now: MonotonicNanos) -> SpringSample {
        let mut displacement = 0.0;
        let mut velocity = 0.0;
        let mut any_moving = false;
        for contribution in &self.contributions {
            if let Some(state) = contribution.live_state_at(now) {
                displacement += state.displacement;
                velocity += state.velocity;
                any_moving = true;
            }
        }
        if any_moving {
            SpringSample {
                value: self.target + displacement,
                velocity,
                phase: SpringPhase::Moving,
            }
        } else {
            SpringSample {
                value: self.target,
                velocity: 0.0,
                phase: SpringPhase::AtRest,
            }
        }
    }

    /// Сменить цель той же пружиной, что вела последний вклад.
    pub fn set_target(&mut self, new_target: f64, now: MonotonicNanos) -> Result<(), SpringError> {
        let spring = self.contributions.last().map_or_else(Spring::smooth, |c| c.spring);
        self.set_target_with(spring, new_target, now)
    }

    /// Сменить цель; новый вклад ведёт `spring`. Для `immediate` все вклады сбрасываются
    /// (значение сразу идёт за целью).
    pub fn set_target_with(&mut self, spring: Spring, new_target: f64, now: MonotonicNanos) -> Result<(), SpringError> {
        ensure_finite("new_target", new_target)?;
        let effective_now = now.max(self.last_event_time);
        self.last_event_time = effective_now;
        // Успокоившиеся вклады выбрасываются: их остаток не больше допуска покоя.
        self.contributions.retain(|c| c.live_state_at(effective_now).is_some());
        let shift = self.target - new_target;
        self.target = new_target;
        if spring.is_immediate() {
            self.contributions.clear();
            return Ok(());
        }
        if shift != 0.0 {
            self.push_contribution(
                spring,
                SpringState {
                    displacement: shift,
                    velocity: 0.0,
                },
                effective_now,
            );
        }
        self.enforce_contribution_limit(effective_now);
        Ok(())
    }

    fn push_contribution(&mut self, spring: Spring, initial: SpringState, start_time: MonotonicNanos) {
        if spring.is_immediate() {
            return;
        }
        let frequency = spring.angular_frequency().unwrap_or(0.0);
        let tolerance = self.rest.tolerance_for_distance(initial.displacement, frequency);
        self.contributions.push(Contribution {
            spring,
            initial,
            start_time,
            tolerance,
        });
    }

    /// Два старейших вклада заменяются одним, равным их сумме в `now` и с пружиной более нового.
    /// Для одинаковых пружин это точно (решение линейно); для разных — приближение.
    fn enforce_contribution_limit(&mut self, now: MonotonicNanos) {
        while self.contributions.len() > self.max_live_contributions && self.contributions.len() >= 2 {
            let oldest = self.contributions.remove(0);
            let next = self.contributions[0];
            let (a, b) = (oldest.state_at(now), next.state_at(now));
            self.contributions[0] = Contribution {
                spring: next.spring,
                initial: SpringState {
                    displacement: a.displacement + b.displacement,
                    velocity: a.velocity + b.velocity,
                },
                start_time: now,
                tolerance: RestTolerance {
                    position: oldest.tolerance.position.max(next.tolerance.position),
                    velocity: oldest.tolerance.velocity.max(next.tolerance.velocity),
                },
            };
        }
    }
}
