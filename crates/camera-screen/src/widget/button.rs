//! Кнопка = узел с `Press` (подсветка и срабатывание, `ui_kit::button::PressTracker`), `Tap` (арбитраж) и
//! пружина нажатия `ui_kit::button::PressAnimation` (плавная степень нажатия для стекла).

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureState, PressSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_core::time::MonotonicNanos;
use ui_draw::draw_list::PressState;
use ui_kit::button::{PressAnimation, PressOutcome, PressTracker};

use super::Ctx;
use super::error::{WidgetError, element};
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;

/// Итог события кнопки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonEvent {
    /// Ничего заметного (подсветка не менялась).
    Idle,
    /// Подсветка включилась/выключилась.
    Highlight,
    /// Палец отпущен на кнопке: сработала.
    Activated,
}

/// Кнопка экрана: узел, механика нажатия и пружина степени нажатия.
pub(crate) struct PressButton {
    node: NodeId,
    tracker: PressTracker,
    /// Плавная степень нажатия: её ведут итоги трекера, читает стекло.
    animation: PressAnimation,
}

impl PressButton {
    /// Создаёт распознаватели `Tap` + `Press` на готовом узле.
    pub(crate) fn attach(
        kit: &mut SceneKit,
        id: ButtonId,
        node: NodeId,
        frame: Rect,
        ctx: &Ctx<'_>,
    ) -> Result<Self, WidgetError> {
        kit.add_recognizer(
            node,
            RecognizerSpec::Tap(TapSpec::default()),
            ScreenGesture::ButtonTapGuard(id),
        )?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Press(PressSpec::default()),
            ScreenGesture::Press(id),
        )?;
        Ok(Self {
            node,
            tracker: PressTracker::new(frame, ctx.tokens).map_err(element("кнопка"))?,
            animation: PressAnimation::new(&ctx.environment(frame.size.width), ctx.now).map_err(element("кнопка"))?,
        })
    }

    pub(crate) fn node(&self) -> NodeId {
        self.node
    }

    /// Рамка кнопки изменилась (значок переехал).
    pub(crate) fn set_frame(&mut self, frame: Rect) -> Result<(), WidgetError> {
        self.tracker.set_frame(frame).map_err(element("кнопка"))
    }

    /// Событие `Press` в момент `time`: трекер решает, нажата ли кнопка, пружина едет к 1 или к 0.
    pub(crate) fn handle(
        &mut self,
        state: GestureState,
        data: GestureData,
        time: MonotonicNanos,
    ) -> Result<ButtonEvent, WidgetError> {
        let outcome = self.tracker.handle_press(state, data).map_err(element("кнопка"))?;
        self.animation.follow(outcome, time).map_err(element("кнопка"))?;
        Ok(match outcome {
            PressOutcome::Unchanged => ButtonEvent::Idle,
            PressOutcome::StateChanged(_) => ButtonEvent::Highlight,
            PressOutcome::Activated => ButtonEvent::Activated,
        })
    }

    /// Нажатие для члена группы стекла в момент `now`: степень плавно едет пружиной `snappy`.
    pub(crate) fn press_state(&self, now: MonotonicNanos) -> PressState {
        self.animation.press_state(now)
    }
}
