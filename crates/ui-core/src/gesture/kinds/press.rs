//! Нажатие кнопки: `Began` сразу на касании, `Changed` при движении, `Ended` на отрыве.
//! Внутри или снаружи кнопки палец — решает элемент `ui-kit` по своей рамке.

use shell_contract::touch::PointerId;

use super::{Input, StepContext, Verdict};
use crate::gesture::spec::PressSpec;
use crate::gesture::state::GestureData;

#[derive(Debug, Clone)]
pub(in crate::gesture) struct PressState {
    spec: PressSpec,
    tracked: Option<PointerId>,
}

impl PressState {
    pub fn new(spec: PressSpec) -> Self {
        Self { spec, tracked: None }
    }

    pub fn spec(&self) -> &PressSpec {
        &self.spec
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone());
    }

    fn data(&self, context: &StepContext<'_>) -> Option<GestureData> {
        let finger = context.fingers.get(self.tracked?)?;
        Some(GestureData::Press {
            position: finger.position,
        })
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        match input {
            Input::Down(pointer) => {
                if self.tracked.is_none() && context.fingers.len() == 1 && context.fingers.contains(pointer) {
                    self.tracked = Some(pointer);
                    if let Some(data) = self.data(context) {
                        return Verdict::Ready(data);
                    }
                }
                Verdict::Idle
            }
            Input::Move(pointer) if self.tracked == Some(pointer) => match self.data(context) {
                Some(data) => Verdict::follow(context.phase, data),
                None => Verdict::Idle,
            },
            Input::Up(pointer) if self.tracked == Some(pointer) => match self.data(context) {
                Some(data) => Verdict::lift(context.phase, data),
                None => Verdict::Fail,
            },
            Input::Move(_) | Input::Up(_) | Input::Tick => Verdict::Idle,
        }
    }
}
