//! Перетаскивание одним пальцем: гистерезис, ось, смещения и скорость.

use shell_contract::touch::PointerId;

use super::{Input, Phase, StepContext, Verdict};
use crate::gesture::spec::PanSpec;
use crate::gesture::state::{GestureData, PanTranslation};

#[derive(Debug, Clone)]
pub(in crate::gesture) struct PanState {
    spec: PanSpec,
    tracked: Option<PointerId>,
    /// Гистерезис превышен и ось подошла: жест готов начаться (может ждать чужого провала).
    triggered: bool,
}

impl PanState {
    pub fn new(spec: PanSpec) -> Self {
        Self {
            spec,
            tracked: None,
            triggered: false,
        }
    }

    pub fn spec(&self) -> &PanSpec {
        &self.spec
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone());
    }

    fn data(&self, context: &StepContext<'_>) -> Option<GestureData> {
        let finger = context.fingers.get(self.tracked?)?;
        Some(GestureData::Pan {
            position: finger.position,
            translation: PanTranslation::from_touch_only(finger.position.offset_from(finger.start_position)),
            velocity: finger.velocity.velocity_at(context.now),
        })
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        match input {
            Input::Down(pointer) => {
                if self.tracked.is_none() && context.fingers.len() == 1 && context.fingers.contains(pointer) {
                    self.tracked = Some(pointer);
                }
                Verdict::Idle
            }
            Input::Move(pointer) if self.tracked == Some(pointer) => {
                let (Some(data), Some(finger)) = (self.data(context), context.fingers.get(pointer)) else {
                    return Verdict::Idle;
                };
                if self.triggered || context.phase == Phase::Active {
                    return Verdict::follow(context.phase, data);
                }
                let displacement = finger.position.offset_from(finger.start_position);
                if displacement.length() <= self.spec.hysteresis_pt {
                    return Verdict::Idle;
                }
                // Гистерезис превышен: ось решается ровно сейчас по доминирующей компоненте.
                if self.spec.direction.accepts(displacement) {
                    self.triggered = true;
                    Verdict::Ready(data)
                } else {
                    Verdict::Fail
                }
            }
            Input::Up(pointer) if self.tracked == Some(pointer) => match self.data(context) {
                Some(data) => Verdict::lift(context.phase, data),
                None => Verdict::Fail,
            },
            Input::Move(_) | Input::Up(_) | Input::Tick => Verdict::Idle,
        }
    }
}
