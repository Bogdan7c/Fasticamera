//! Механика затвора: нажатие через `PressTracker`, анимация нажатия, вид от экрана.

use ui_core::geometry::Rect;
use ui_core::gesture::{GestureData, GestureState};
use ui_core::time::{MonotonicNanos, MotionPreference};

use super::error::ShutterError;
use crate::button::{ButtonState, PressOutcome, PressTracker};
use crate::environment::ElementEnvironment;
use crate::progress::{SpringProgress, SpringSource};
use crate::tokens::MotionToken;

/// Внешний вид затвора, который назначает экран по своей машине состояний (М2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutterLook {
    /// Готов к съёмке: белый диск в кольце.
    Ready,
    /// Удерживается в нажатом виде независимо от пальца (например, идёт серия): диск сжат.
    Pressed,
    /// Занят (идёт обработка): диск притушен, нажатие всё ещё принимается.
    Busy,
    /// Недоступен: затвор притушен и **не срабатывает** (нажатие игнорируется).
    Disabled,
}

/// Срез состояния затвора для рисования и проверок.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShutterSnapshot {
    pub look: ShutterLook,
    /// Сила нажатия 0..1 (анимированная): палец на кнопке или вид `Pressed`.
    pub press_amount: f32,
    /// Масштаб белого диска: 1 в покое, `pressed_disc_scale` при полном нажатии.
    pub disc_scale: f64,
}

/// Затвор.
#[derive(Debug, Clone)]
pub struct ShutterButton {
    tracker: PressTracker,
    look: ShutterLook,
    pressed_disc_scale: f64,
    springs: SpringSource,
    /// 0 — отпущен, 1 — нажат.
    press: SpringProgress,
}

impl ShutterButton {
    /// Пружина нажатия и отпускания: `.smooth` без перелёта.
    const PRESS_SPRING: MotionToken = MotionToken::Smooth;

    /// Создать затвор с рамкой `frame` (pt) и начальным видом.
    pub fn new(
        frame: Rect,
        look: ShutterLook,
        environment: &ElementEnvironment<'_>,
        now: MonotonicNanos,
    ) -> Result<Self, ShutterError> {
        let tracker = PressTracker::new(frame, environment.tokens)?;
        let springs = SpringSource::new(&environment.tokens.motion, environment.motion);
        let press = SpringProgress::at_rest(
            if look == ShutterLook::Pressed { 1.0 } else { 0.0 },
            springs.spring(Self::PRESS_SPRING)?,
            now,
        )?;
        Ok(Self {
            tracker,
            look,
            pressed_disc_scale: environment.tokens.shutter.pressed_disc_scale,
            springs,
            press,
        })
    }

    pub fn look(&self) -> ShutterLook {
        self.look
    }

    /// Рамка затвора изменилась (вёрстка).
    pub fn set_frame(&mut self, frame: Rect) -> Result<(), ShutterError> {
        Ok(self.tracker.set_frame(frame)?)
    }

    /// Назначить вид. Недоступный затвор сразу отпускается (идущее нажатие отменяется без срабатывания).
    pub fn set_look(&mut self, look: ShutterLook, now: MonotonicNanos) -> Result<(), ShutterError> {
        self.look = look;
        if look == ShutterLook::Disabled {
            self.tracker.handle_press(GestureState::Cancelled, GestureData::None)?;
        }
        self.sync_press_target(now)
    }

    /// Событие распознавателя `Press` затвора. `time` — время события.
    ///
    /// Недоступный затвор ничего не принимает: итог всегда [`PressOutcome::Unchanged`].
    pub fn handle_press(
        &mut self,
        state: GestureState,
        data: GestureData,
        time: MonotonicNanos,
    ) -> Result<PressOutcome, ShutterError> {
        if self.look == ShutterLook::Disabled {
            return Ok(PressOutcome::Unchanged);
        }
        let outcome = self.tracker.handle_press(state, data)?;
        if outcome != PressOutcome::Unchanged {
            self.sync_press_target(time)?;
        }
        Ok(outcome)
    }

    /// Цель анимации нажатия: палец на кнопке или вид `Pressed`.
    fn sync_press_target(&mut self, now: MonotonicNanos) -> Result<(), ShutterError> {
        let held = self.tracker.state() == ButtonState::Pressed || self.look == ShutterLook::Pressed;
        let target = if held { 1.0 } else { 0.0 };
        if (self.press.target() - target).abs() > f64::EPSILON {
            let spring = self.springs.spring(Self::PRESS_SPRING)?;
            self.press.retarget(spring, target, now)?;
        }
        Ok(())
    }

    /// Reduce Motion включили или выключили на лету.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.springs.set_motion(motion);
    }

    /// Всё для рисования за один вызов.
    pub fn snapshot(&self, now: MonotonicNanos) -> ShutterSnapshot {
        let amount = self.press.value(now).clamp(0.0, 1.0);
        ShutterSnapshot {
            look: self.look,
            press_amount: amount as f32,
            disc_scale: 1.0 + (self.pressed_disc_scale - 1.0) * amount,
        }
    }
}
