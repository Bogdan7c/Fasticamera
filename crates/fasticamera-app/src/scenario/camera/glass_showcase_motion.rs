//! Движение стенда по событиям расписания: нажатия (число `amount` по пружине) и команды сцене.
//!
//! Своей физики здесь нет: пружины — из токенов `ui-kit` (`MotionTokens::spring`), решатель — `ui-core`
//! (`SpringAnimation`). Нажатие кнопки — не свойство узла, а число 0..1 для `PressState`, поэтому едет
//! отдельной `SpringAnimation`; кружки зума и панель — свойства узлов, едут аниматором сцены.

use ui_core::spring::{RestCriterion, Spring, SpringAnimation, SpringStart};
use ui_core::time::MonotonicNanos;
use ui_kit::tokens::{MotionToken, UiKitTokens};

use super::error::ShowcaseError;
use super::scene::{PanelState, ShowcaseScene, ZoomSpread};
use super::timeline::{ShowcaseTarget, TimelineEvent, WindowEdge};

/// Нажатие одной кнопки: пружина `amount` к 1 (нажата) или 0 (отпущена).
#[derive(Debug, Clone, Copy)]
struct PressDrive {
    animation: SpringAnimation,
}

impl PressDrive {
    /// Кнопка в покое (не нажата) с момента `since`.
    fn at_rest(spring: Spring, rest: RestCriterion, since: MonotonicNanos) -> Result<Self, ShowcaseError> {
        let animation = SpringAnimation::start(SpringStart {
            spring,
            from: 0.0,
            to: 0.0,
            initial_velocity: 0.0,
            start_time: since,
            rest,
        })?;
        Ok(Self { animation })
    }

    /// Нажать (`true`) или отпустить: перенацеливание от текущих положения и скорости.
    fn set_pressed(&mut self, pressed: bool, now: MonotonicNanos) -> Result<(), ShowcaseError> {
        let target = if pressed { 1.0 } else { 0.0 };
        self.animation.retarget(target, now)?;
        Ok(())
    }

    /// Нажатие в момент `now`, зажатое в 0..1: пружина `snappy` слегка перелетает цель, а
    /// `PressState.amount` по словарю рисования обязан лежать в 0..1.
    fn amount(&self, now: MonotonicNanos) -> f32 {
        self.animation.sample(now).value.clamp(0.0, 1.0) as f32
    }
}

/// Нажатия кнопок в момент кадра.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PressAmounts {
    pub flash: f32,
    pub flip: f32,
}

/// Двигатель стенда: пружины и состояние нажатий.
pub(super) struct ShowcaseMotion {
    snappy: Spring,
    sheet: Spring,
    rest: RestCriterion,
    flash: PressDrive,
    flip: PressDrive,
}

impl ShowcaseMotion {
    /// Пружины из токенов: `snappy` — нажатие и зум, `camera.sheet` — панель (`motion.md`).
    pub(super) fn new(
        tokens: &UiKitTokens,
        motion: ui_core::time::MotionPreference,
        unit_rest_floor: f64,
        since: MonotonicNanos,
    ) -> Result<Self, ShowcaseError> {
        let snappy = tokens.motion.spring(MotionToken::Snappy, motion)?;
        let sheet = tokens.motion.spring(MotionToken::CameraSheet, motion)?;
        let rest = RestCriterion {
            absolute_position_floor: unit_rest_floor,
            ..RestCriterion::default()
        };
        Ok(Self {
            snappy,
            sheet,
            rest,
            flash: PressDrive::at_rest(snappy, rest, since)?,
            flip: PressDrive::at_rest(snappy, rest, since)?,
        })
    }

    /// Вернуть нажатия в покой с момента `since` (сцена при этом пересоздаётся снаружи).
    pub(super) fn reset(&mut self, since: MonotonicNanos) -> Result<(), ShowcaseError> {
        self.flash = PressDrive::at_rest(self.snappy, self.rest, since)?;
        self.flip = PressDrive::at_rest(self.snappy, self.rest, since)?;
        Ok(())
    }

    /// Исполнить событие расписания в момент `at` (время СОБЫТИЯ, а не кадра: так результат не зависит
    /// от частоты кадров).
    pub(super) fn handle(
        &mut self,
        scene: &mut ShowcaseScene,
        event: TimelineEvent,
        at: MonotonicNanos,
    ) -> Result<(), ShowcaseError> {
        let entering = event.edge == WindowEdge::Enter;
        match event.target {
            ShowcaseTarget::FlashPress => self.flash.set_pressed(entering, at),
            ShowcaseTarget::FlipPress => self.flip.set_pressed(entering, at),
            ShowcaseTarget::ZoomApart => {
                let spread = if entering {
                    ZoomSpread::Apart
                } else {
                    ZoomSpread::Together
                };
                scene.animate_zoom_spread(spread, self.snappy, at)
            }
            ShowcaseTarget::PanelOpen => {
                let state = if entering {
                    PanelState::Open
                } else {
                    PanelState::Collapsed
                };
                scene.animate_panel(state, self.sheet, at)
            }
        }
    }

    /// Нажатия кнопок в момент `now`.
    pub(super) fn press_amounts(&self, now: MonotonicNanos) -> PressAmounts {
        PressAmounts {
            flash: self.flash.amount(now),
            flip: self.flip.amount(now),
        }
    }
}
