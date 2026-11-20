//! Вход и выход одного тика исполнителя `zoom` (сессия 13): наблюдение, что вложить в экран и в каком состоянии прогон.

use std::time::Instant;

use camera_core::CameraEvent;
use camera_screen::ScreenAction;
use render_loop::viewfinder::{ContinuityCounters, FrameRecord};
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::TouchPhase;

use super::zoom_views::{EngineView, ScreenView};
use crate::screen_scenario_config::ScreenPoint;

/// Один тик наблюдения.
pub struct ZoomObservation<'a> {
    /// Часы маршрутизатора.
    pub now: Instant,
    /// Самое свежее время на шкале рендера (кадры видоискателя/UI): им метятся действия и касания.
    pub render_now: MonotonicNanos,
    pub engine: &'a EngineView,
    /// События движка, пришедшие с прошлого тика (исполнитель — единственный, кто их забирает).
    pub events: &'a [CameraEvent],
    /// Кольцо последних показанных кадров видоискателя (старые первыми).
    pub frames: &'a [FrameRecord],
    pub counters: &'a ContinuityCounters,
    pub screen: &'a ScreenView,
}

/// Образец касания в долях экрана (сценарий-обвязка переводит в пиксели поверхности).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchInjection {
    pub phase: TouchPhase,
    pub point: ScreenPoint,
    pub time: MonotonicNanos,
}

/// Что вложить в экран.
#[derive(Debug, Clone, PartialEq)]
pub enum Injection {
    Action(ScreenAction),
    Touch(TouchInjection),
}

/// Состояние прогона после тика.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerState {
    Running,
    AllDone,
    Failed(String),
}

/// Итог тика.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomTick {
    pub injections: Vec<Injection>,
    pub state: RunnerState,
}

impl ZoomTick {
    pub fn idle(state: RunnerState) -> Self {
        Self {
            injections: Vec::new(),
            state,
        }
    }
}
