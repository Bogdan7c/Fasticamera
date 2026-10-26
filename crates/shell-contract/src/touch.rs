//! Касания экрана в нейтральном виде.
//!
//! Одна запись — одно положение одного пальца в один момент. Координаты — пиксели surface
//! (перевод в точки интерфейса — забота `ui-core`), время — `CLOCK_MONOTONIC` (шкала
//! `MotionEvent`, см. [`crate::clock`]).

use serde::Serialize;

use crate::clock::MonotonicNanos;

/// Номер пальца. Постоянен, пока палец касается экрана; после отрыва может достаться другому.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PointerId(i32);

impl PointerId {
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> i32 {
        self.0
    }
}

/// Фаза касания для конкретного пальца.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    /// Палец коснулся экрана.
    Began,
    /// Палец сдвинулся (в том числе промежуточные точки, накопленные системой между кадрами).
    Moved,
    /// Палец оторвался от экрана.
    Ended,
    /// Система отменила касание (например, жест ушёл системе) — жест нужно сбросить без действия.
    Cancelled,
}

/// Точка в пикселях surface; начало координат — левый верхний угол.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SurfacePoint {
    pub x_px: f32,
    pub y_px: f32,
}

/// Одно наблюдение одного пальца.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TouchSample {
    pub pointer: PointerId,
    pub phase: TouchPhase,
    pub position: SurfacePoint,
    pub time: MonotonicNanos,
}
