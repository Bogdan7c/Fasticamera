//! Свайп: дискретный, решается по ходу движения (не ждёт отрыва).

use shell_contract::touch::PointerId;

use super::{Input, StepContext, Verdict};
use crate::geometry::Vector;
use crate::gesture::spec::{SwipeSpec, millis};
use crate::gesture::state::{GestureData, SwipeDirection};
use crate::time::{MonotonicNanos, UiDuration};
use crate::velocity::VelocityEstimate;

#[derive(Debug, Clone)]
pub(in crate::gesture) struct SwipeState {
    spec: SwipeSpec,
    tracked: Option<PointerId>,
    started_at: Option<MonotonicNanos>,
    /// Свайп признан; вид молчит, ждёт решения арены.
    triggered: bool,
}

/// Доминирующее направление смещения (при ничьей — горизонталь) и его длина.
fn dominant_direction(displacement: Vector) -> (SwipeDirection, f64) {
    if displacement.dx.abs() >= displacement.dy.abs() {
        let direction = if displacement.dx < 0.0 {
            SwipeDirection::Left
        } else {
            SwipeDirection::Right
        };
        (direction, displacement.dx.abs())
    } else {
        let direction = if displacement.dy < 0.0 {
            SwipeDirection::Up
        } else {
            SwipeDirection::Down
        };
        (direction, displacement.dy.abs())
    }
}

/// Скорость вдоль направления (положительна, если палец едет в эту сторону).
fn speed_along(direction: SwipeDirection, velocity: Vector) -> f64 {
    match direction {
        SwipeDirection::Left => -velocity.dx,
        SwipeDirection::Right => velocity.dx,
        SwipeDirection::Up => -velocity.dy,
        SwipeDirection::Down => velocity.dy,
    }
}

impl SwipeState {
    pub fn new(spec: SwipeSpec) -> Self {
        Self {
            spec,
            tracked: None,
            started_at: None,
            triggered: false,
        }
    }

    pub fn spec(&self) -> &SwipeSpec {
        &self.spec
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone());
    }

    fn out_of_time(&self, now: MonotonicNanos) -> bool {
        self.started_at
            .is_some_and(|started| UiDuration::between(started, now) > millis(self.spec.max_duration_ms))
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        if self.triggered {
            return Verdict::Idle;
        }
        match input {
            Input::Down(pointer) => {
                if self.tracked.is_none() && context.fingers.len() == 1 && context.fingers.contains(pointer) {
                    self.tracked = Some(pointer);
                    self.started_at = Some(context.now);
                }
                Verdict::Idle
            }
            Input::Move(pointer) if self.tracked == Some(pointer) => self.on_move(pointer, context),
            Input::Up(pointer) if self.tracked == Some(pointer) => Verdict::Fail,
            Input::Tick if self.tracked.is_some() && self.out_of_time(context.now) => Verdict::Fail,
            Input::Move(_) | Input::Up(_) | Input::Tick => Verdict::Idle,
        }
    }

    fn on_move(&mut self, pointer: PointerId, context: &StepContext<'_>) -> Verdict {
        if self.out_of_time(context.now) {
            return Verdict::Fail;
        }
        let Some(finger) = context.fingers.get(pointer) else {
            return Verdict::Idle;
        };
        let (direction, distance) = dominant_direction(finger.position.offset_from(finger.start_position));
        if distance < self.spec.min_distance_pt {
            return Verdict::Idle;
        }
        if !self.spec.directions.contains(&direction) {
            return Verdict::Fail;
        }
        // Дистанции мало: скорость решает, свайп это или медленное перетаскивание.
        let velocity = match finger.velocity.estimate_at(context.now) {
            VelocityEstimate::Measured(velocity) => velocity,
            VelocityEstimate::NotEnoughSamples | VelocityEstimate::Stopped => return Verdict::Idle,
        };
        if speed_along(direction, velocity) < self.spec.min_velocity_pt_per_s {
            return Verdict::Idle;
        }
        self.triggered = true;
        Verdict::Ready(GestureData::Swipe {
            direction,
            position: finger.position,
            velocity,
        })
    }
}
