//! Долгий тап: непрерывный; после `Began` палец можно вести (колесо зума тем же касанием).

use shell_contract::touch::PointerId;

use super::{Input, StepContext, Verdict};
use crate::geometry::Point;
use crate::gesture::spec::{LongPressSpec, millis};
use crate::gesture::state::{GestureData, PanTranslation};
use crate::time::{MonotonicNanos, UiDuration};

#[derive(Debug, Clone)]
pub(in crate::gesture) struct LongPressState {
    spec: LongPressSpec,
    tracked: Option<PointerId>,
    start: Option<(Point, MonotonicNanos)>,
    /// Время вышло, палец не двигался: жест готов начаться (может ждать чужого провала).
    triggered: bool,
}

impl LongPressState {
    pub fn new(spec: LongPressSpec) -> Self {
        Self {
            spec,
            tracked: None,
            start: None,
            triggered: false,
        }
    }

    pub fn spec(&self) -> &LongPressSpec {
        &self.spec
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone());
    }

    fn data(&self, context: &StepContext<'_>) -> Option<GestureData> {
        let finger = context.fingers.get(self.tracked?)?;
        Some(GestureData::LongPress {
            position: finger.position,
            translation: PanTranslation::from_touch_only(finger.position.offset_from(finger.start_position)),
        })
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        match input {
            Input::Down(pointer) => {
                if self.tracked.is_none()
                    && context.fingers.len() == 1
                    && let Some(finger) = context.fingers.get(pointer)
                {
                    self.tracked = Some(pointer);
                    self.start = Some((finger.position, context.now));
                }
                Verdict::Idle
            }
            Input::Move(pointer) if self.tracked == Some(pointer) => {
                let Some(data) = self.data(context) else {
                    return Verdict::Idle;
                };
                if self.triggered || context.phase == super::Phase::Active {
                    return Verdict::follow(context.phase, data);
                }
                let moved_too_far = match (self.start, context.fingers.get(pointer)) {
                    (Some((start_position, _)), Some(finger)) => {
                        finger.position.distance_to(start_position) > self.spec.allowable_movement_pt
                    }
                    _ => false,
                };
                if moved_too_far { Verdict::Fail } else { Verdict::Idle }
            }
            Input::Up(pointer) if self.tracked == Some(pointer) => match self.data(context) {
                Some(data) if context.phase == super::Phase::Active => Verdict::Finish(data),
                _ => Verdict::Fail,
            },
            Input::Tick => {
                let held_long_enough = !self.triggered
                    && self.tracked.is_some()
                    && self.start.is_some_and(|(_, started)| {
                        UiDuration::between(started, context.now) >= millis(self.spec.min_duration_ms)
                    });
                match (held_long_enough, self.data(context)) {
                    (true, Some(data)) => {
                        self.triggered = true;
                        Verdict::Ready(data)
                    }
                    _ => Verdict::Idle,
                }
            }
            Input::Move(_) | Input::Up(_) => Verdict::Idle,
        }
    }
}
