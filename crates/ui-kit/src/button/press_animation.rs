//! [`PressAnimation`] — плавная степень нажатия обычной кнопки (K2) для стекла.
//!
//! [`PressTracker`](super::PressTracker) отвечает «нажата или нет» и меняет ответ скачком; стекло же
//! ждёт `PressState.amount` — число 0..1, которое едет к цели пружиной (в демо стекла — `snappy`,
//! `glass.md`). Этот элемент не знает о жестах: его ведут итоги трекера ([`PressOutcome`]), поэтому
//! подсветка, уход пальца, возврат и системная отмена автоматически дают нужное направление.
//!
//! Физики здесь нет: это [`SpringProgress`] из `ui-core` и пружина-токен [`MotionToken::Snappy`].
//! Затвор (K10) — отдельный случай с пружиной `smooth`: ему нужен «спокойный» диск без перелёта.
//!
//! # Перелёт
//! `snappy` слегка перелетает цель. Контракт стекла требует `amount` в 0..1, поэтому
//! [`PressAnimation::amount`] зажимает значение: при отпускании ниже нуля не уходит, при нажатии
//! выше единицы не поднимается. Reduce Motion убирает перелёт на уровне пружины (правило токенов).

use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_draw::draw_list::PressState;

use super::press_tracker::{ButtonError, ButtonState, PressOutcome};
use crate::environment::ElementEnvironment;
use crate::progress::{SpringProgress, SpringSource};
use crate::tokens::MotionToken;

/// Анимированная степень нажатия одной кнопки.
#[derive(Debug, Clone)]
pub struct PressAnimation {
    springs: SpringSource,
    /// 0 — отпущена, 1 — нажата.
    press: SpringProgress,
}

impl PressAnimation {
    /// Пружина нажатия и отпускания (как в демо стекла, `glass.md`).
    const PRESS_SPRING: MotionToken = MotionToken::Snappy;

    /// Кнопка в покое (отпущена) с момента `now`.
    pub fn new(environment: &ElementEnvironment<'_>, now: MonotonicNanos) -> Result<Self, ButtonError> {
        environment.tokens.validate()?;
        let springs = SpringSource::new(&environment.tokens.motion, environment.motion);
        let press = SpringProgress::at_rest(0.0, springs.spring(Self::PRESS_SPRING)?, now)?;
        Ok(Self { springs, press })
    }

    /// Применить итог очередного события `Press` трекера. `time` — время этого события.
    ///
    /// * подсветка включилась — цель 1; выключилась (увёл палец, отмена) или кнопка сработала — цель 0;
    /// * [`PressOutcome::Unchanged`] ничего не трогает: идущая анимация не перезапускается.
    ///
    /// Перенацеливание наследует положение и скорость, поэтому быстрый «нажал — отпустил» не даёт скачка.
    pub fn follow(&mut self, outcome: PressOutcome, time: MonotonicNanos) -> Result<(), ButtonError> {
        let target = match outcome {
            PressOutcome::Unchanged => return Ok(()),
            PressOutcome::StateChanged(ButtonState::Pressed) => 1.0,
            PressOutcome::StateChanged(ButtonState::Normal) | PressOutcome::Activated => 0.0,
        };
        if (self.press.target() - target).abs() > f64::EPSILON {
            let spring = self.springs.spring(Self::PRESS_SPRING)?;
            self.press.retarget(spring, target, time)?;
        }
        Ok(())
    }

    /// Степень нажатия в момент `now`, строго в 0..1.
    pub fn amount(&self, now: MonotonicNanos) -> f32 {
        self.press.value(now).clamp(0.0, 1.0) as f32
    }

    /// Нажатие для члена группы стекла: степень в момент `now`, точка касания в центре.
    pub fn press_state(&self, now: MonotonicNanos) -> PressState {
        PressState {
            amount: self.amount(now),
            ..PressState::RELEASED
        }
    }

    /// Анимация ещё едет: экрану нужен следующий кадр.
    pub fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.press.is_moving(now)
    }

    /// Reduce Motion включили или выключили на лету; действует на новые перенацеливания.
    pub fn set_motion(&mut self, motion: MotionPreference) {
        self.springs.set_motion(motion);
    }
}
