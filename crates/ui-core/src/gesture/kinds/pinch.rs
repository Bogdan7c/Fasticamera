//! Щипок двумя пальцами: масштаб, скорость масштаба, фокус.
//!
//! Пара — первые два пальца касания. Третий палец игнорируется. Отрыв любого из пары заканчивает
//! щипок (`Ended`); оставшийся палец ничего не начинает — однопальцевые жесты уже провалены вторым
//! пальцем, а сброс происходит только после отрыва всех пальцев.

use shell_contract::touch::PointerId;

use super::{Input, Phase, StepContext, Verdict};
use crate::gesture::spec::PinchSpec;
use crate::gesture::state::GestureData;
use crate::velocity::ScalarVelocityTracker;

/// Расстояние между пальцами при касании меньше этого (pt) не даёт осмысленного масштаба
/// (деление на ~0): щипок проваливается. Технический порог против деления на ноль, не пользовательский.
const MIN_INITIAL_DISTANCE_PT: f64 = 1e-3;

#[derive(Debug, Clone)]
pub(in crate::gesture) struct PinchState {
    spec: PinchSpec,
    pair: Option<(PointerId, PointerId)>,
    initial_distance: f64,
    triggered: bool,
    /// История отношения «расстояние / начальное расстояние» для скорости масштаба.
    scale_history: ScalarVelocityTracker,
    scale_history_template: ScalarVelocityTracker,
}

impl PinchState {
    pub fn new(spec: PinchSpec, scale_history: ScalarVelocityTracker) -> Self {
        Self {
            spec,
            pair: None,
            initial_distance: 0.0,
            triggered: false,
            scale_history: scale_history.clone(),
            scale_history_template: scale_history,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone(), self.scale_history_template.clone());
    }

    fn pair_distance(&self, context: &StepContext<'_>) -> Option<(f64, GestureData)> {
        let (first_id, second_id) = self.pair?;
        let first = context.fingers.get(first_id)?;
        let second = context.fingers.get(second_id)?;
        let distance = first.position.distance_to(second.position);
        let scale = distance / self.initial_distance;
        let data = GestureData::Pinch {
            scale,
            scale_velocity: self.scale_history.estimate_at(context.now).or_zero(),
            focus: first.position.midpoint(second.position),
        };
        Some((distance, data))
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        match input {
            Input::Down(_) => self.on_down(context),
            Input::Move(pointer) if self.in_pair(pointer) => self.on_move(context),
            Input::Up(pointer) if self.in_pair(pointer) => {
                // Отрыв не добавляет выборку масштаба: берём данные по последним положениям.
                self.pair_distance(context)
                    .map_or(Verdict::Fail, |(_, data)| Verdict::lift(context.phase, data))
            }
            // Однопальцевый отрыв до появления пары: щипка уже не будет.
            Input::Up(_) if self.pair.is_none() => Verdict::Fail,
            Input::Move(_) | Input::Up(_) | Input::Tick => Verdict::Idle,
        }
    }

    fn in_pair(&self, pointer: PointerId) -> bool {
        self.pair
            .is_some_and(|(first, second)| first == pointer || second == pointer)
    }

    fn on_down(&mut self, context: &StepContext<'_>) -> Verdict {
        if self.pair.is_some() {
            return Verdict::Idle;
        }
        let Some((first, second)) = context.fingers.first_two() else {
            return Verdict::Idle;
        };
        let distance = first.position.distance_to(second.position);
        if distance < MIN_INITIAL_DISTANCE_PT {
            return Verdict::Fail;
        }
        self.pair = Some((first.pointer, second.pointer));
        self.initial_distance = distance;
        self.scale_history.add_sample(context.now, 1.0);
        Verdict::Idle
    }

    /// Данные жеста с учётом новой выборки масштаба (для `Move`).
    fn on_move_data(&mut self, context: &StepContext<'_>) -> Option<GestureData> {
        let (distance, _) = self.pair_distance(context)?;
        self.scale_history
            .add_sample(context.now, distance / self.initial_distance);
        self.pair_distance(context).map(|(_, data)| data)
    }

    fn on_move(&mut self, context: &StepContext<'_>) -> Verdict {
        let Some(data) = self.on_move_data(context) else {
            return Verdict::Idle;
        };
        if self.triggered || context.phase == Phase::Active {
            return Verdict::follow(context.phase, data);
        }
        let Some((distance, _)) = self.pair_distance(context) else {
            return Verdict::Idle;
        };
        if (distance - self.initial_distance).abs() >= self.spec.start_distance_change_pt {
            self.triggered = true;
            Verdict::Ready(data)
        } else {
            Verdict::Idle
        }
    }
}
