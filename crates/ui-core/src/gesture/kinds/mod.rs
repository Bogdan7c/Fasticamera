//! Рабочие состояния распознавателей по видам.
//!
//! Каждый вид — маленький автомат: получает событие пальцев и возвращает [`Verdict`], не зная ни о
//! других распознавателях, ни об арбитраже. Арбитраж (кто победил, кого провалить) — в арене.

mod long_press;
mod pan;
mod pinch;
mod press;
mod swipe;
mod tap;

use shell_contract::touch::PointerId;

use super::spec::{ExtraFingers, RecognizerSpec};
use super::state::GestureData;
use super::touches::Fingers;
use crate::time::MonotonicNanos;
use crate::velocity::ScalarVelocityTracker;

/// Что произошло с пальцами (или со временем).
#[derive(Debug, Clone, Copy)]
pub(super) enum Input {
    /// Палец лёг (уже добавлен в [`Fingers`]).
    Down(PointerId),
    /// Палец сдвинулся (положение уже обновлено).
    Move(PointerId),
    /// Палец оторвался (ещё лежит в [`Fingers`] до конца обработки).
    Up(PointerId),
    /// Время продвинулось: кадр или метка касания.
    Tick,
}

/// В какой фазе сейчас распознаватель.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// `Possible`: ещё не начал (в том числе «готов, но ждёт чужого провала»).
    Possible,
    /// `Began`/`Changed`.
    Active,
}

pub(super) struct StepContext<'a> {
    pub fingers: &'a Fingers,
    pub now: MonotonicNanos,
    pub phase: Phase,
}

/// Ответ вида на событие.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Verdict {
    /// Ничего нового.
    Idle,
    /// (`Possible`) условия начала выполнены. Для непрерывного — быть `Began`, для дискретного —
    /// `Ended`; когда именно — решает арена (отношения «требует провала»). Вид может повторять
    /// `Ready` с обновлёнными данными, пока ждёт.
    Ready(GestureData),
    /// (`Active`) жест продолжается: `Changed`.
    Update(GestureData),
    /// (`Active`) жест закончился нормально: `Ended`.
    Finish(GestureData),
    /// (`Possible`) жест не получится: `Failed`.
    Fail,
}

impl Verdict {
    /// Общий ответ на движение уже сработавшего жеста: активный обновляется, ждущий —
    /// обновляет данные для будущего `Began`.
    pub(super) fn follow(phase: Phase, data: GestureData) -> Self {
        match phase {
            Phase::Active => Self::Update(data),
            Phase::Possible => Self::Ready(data),
        }
    }

    /// Общий ответ на отрыв: активный заканчивается, невыстреливший проваливается.
    pub(super) fn lift(phase: Phase, data: GestureData) -> Self {
        match phase {
            Phase::Active => Self::Finish(data),
            Phase::Possible => Self::Fail,
        }
    }
}

/// Вид и его рабочее состояние.
#[derive(Debug, Clone)]
pub(super) enum Kind {
    Tap(tap::TapState),
    LongPress(long_press::LongPressState),
    Pan(pan::PanState),
    Pinch(pinch::PinchState),
    Swipe(swipe::SwipeState),
    Press(press::PressState),
}

impl Kind {
    /// `scale_velocity` — пустой оценщик для скорости масштаба щипка.
    pub fn new(spec: &RecognizerSpec, scale_velocity: &ScalarVelocityTracker) -> Self {
        match spec {
            RecognizerSpec::Tap(spec) => Self::Tap(tap::TapState::new(spec.clone())),
            RecognizerSpec::LongPress(spec) => Self::LongPress(long_press::LongPressState::new(spec.clone())),
            RecognizerSpec::Pan(spec) => Self::Pan(pan::PanState::new(spec.clone())),
            RecognizerSpec::Pinch(spec) => Self::Pinch(pinch::PinchState::new(spec.clone(), scale_velocity.clone())),
            RecognizerSpec::Swipe(spec) => Self::Swipe(swipe::SwipeState::new(spec.clone())),
            RecognizerSpec::Press(spec) => Self::Press(press::PressState::new(spec.clone())),
        }
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        match self {
            Self::Tap(state) => state.step(input, context),
            Self::LongPress(state) => state.step(input, context),
            Self::Pan(state) => state.step(input, context),
            Self::Pinch(state) => state.step(input, context),
            Self::Swipe(state) => state.step(input, context),
            Self::Press(state) => state.step(input, context),
        }
    }

    /// Вернуть рабочее состояние в исходное (после терминального состояния и отрыва всех пальцев).
    pub fn reset(&mut self) {
        match self {
            Self::Tap(state) => state.reset(),
            Self::LongPress(state) => state.reset(),
            Self::Pan(state) => state.reset(),
            Self::Pinch(state) => state.reset(),
            Self::Swipe(state) => state.reset(),
            Self::Press(state) => state.reset(),
        }
    }

    /// Ждёт ли вид продолжения за пределами текущего касания (второй тап двойного). Только такие
    /// виды (и ждущие чужого провала) переживают полный отрыв пальцев.
    pub fn awaits_more_input(&self) -> bool {
        match self {
            Self::Tap(state) => state.awaits_next_tap(),
            Self::LongPress(_) | Self::Pan(_) | Self::Pinch(_) | Self::Swipe(_) | Self::Press(_) => false,
        }
    }

    pub fn extra_fingers(&self) -> Option<ExtraFingers> {
        match self {
            Self::Tap(state) => Some(state.spec().extra_fingers),
            Self::LongPress(state) => Some(state.spec().extra_fingers),
            Self::Pan(state) => Some(state.spec().extra_fingers),
            Self::Swipe(state) => Some(state.spec().extra_fingers),
            Self::Press(state) => Some(state.spec().extra_fingers),
            Self::Pinch(_) => None,
        }
    }
}
