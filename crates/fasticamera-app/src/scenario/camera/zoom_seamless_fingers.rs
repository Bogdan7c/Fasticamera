//! Пальцы бесшовных шагов `zoom` (13b): где лежат и как двигаются, в виде настоящих образцов касаний.
//!
//! Чистая модель без часов и ввода-вывода. Она не знает, какой коэффициент зума получится: сколько пикселей
//! колеса на единицу зума и как щипок переводится в масштаб, знает экран. Поэтому модель умеет только
//! «подвинь пальцы в сторону приближения/отдаления на один шаг»; решение «дальше или хватит» принимает
//! вызывающий по тому, что показывает экран.
//!
//! Метки времени образцов строго возрастают (экран считает долгое нажатие и скорость по меткам): каждая метка
//! берётся из текущего времени рендера, но не меньше предыдущей плюс 1 нс.

use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, TouchPhase};

use super::zoom_plan::SeamlessGesture;
use super::zoom_tick::TouchInjection;
use crate::screen_scenario_config::ScreenPoint;
use crate::zoom_scenario_config::SeamlessPlan;

/// Нижняя и верхняя границы половины расстояния между пальцами щипка (доли ширины): пальцы не сходятся в точку и
/// не уходят за край экрана.
const MIN_HALF_WIDTH: f64 = 0.02;
const MAX_HALF_WIDTH: f64 = 0.48;

/// Куда двигать значение зума.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomDirection {
    /// Приблизить (коэффициент растёт): колесо — палец влево, щипок — пальцы врозь.
    In,
    /// Отдалить (коэффициент падает): колесо — палец вправо, щипок — пальцы навстречу.
    Out,
}

/// Положение пальцев жеста.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Placement {
    /// Один палец, который ведёт колесо.
    Wheel { point: ScreenPoint },
    /// Два пальца по горизонтали вокруг центра.
    Pinch { center: ScreenPoint, half_width: f64 },
}

/// Пальцы лежащего (или готового лечь) жеста.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fingers {
    placement: Placement,
    last_time: MonotonicNanos,
}

impl Fingers {
    /// Пальцы нужного жеста в исходном положении (ещё не на экране). `strip_point` — куда ложится палец долгого
    /// нажатия: центр ленты упоров (та же точка, что у протяжки `wheel_drag`).
    pub fn new(gesture: SeamlessGesture, plan: &SeamlessPlan, strip_point: ScreenPoint) -> Self {
        let placement = match gesture {
            SeamlessGesture::Wheel => Placement::Wheel { point: strip_point },
            SeamlessGesture::Pinch => Placement::Pinch {
                center: plan.pinch_center,
                half_width: plan.pinch_start_half_width,
            },
        };
        Self {
            placement,
            last_time: MonotonicNanos::from_nanos(0),
        }
    }

    /// Какой жест ведут эти пальцы.
    pub fn gesture(&self) -> SeamlessGesture {
        match self.placement {
            Placement::Wheel { .. } => SeamlessGesture::Wheel,
            Placement::Pinch { .. } => SeamlessGesture::Pinch,
        }
    }

    /// Пальцы ложатся на экран.
    pub fn press(&mut self, now: MonotonicNanos) -> Vec<TouchInjection> {
        self.samples(TouchPhase::Began, now)
    }

    /// Пальцы остаются на месте (образец `Moved` без сдвига: жест жив, значение не меняется).
    pub fn hold(&mut self, now: MonotonicNanos) -> Vec<TouchInjection> {
        self.samples(TouchPhase::Moved, now)
    }

    /// Пальцы поднимаются.
    pub fn lift(&mut self, now: MonotonicNanos) -> Vec<TouchInjection> {
        self.samples(TouchPhase::Ended, now)
    }

    /// Один шаг в сторону `direction` и образцы нового положения.
    pub fn nudge(&mut self, direction: ZoomDirection, plan: &SeamlessPlan, now: MonotonicNanos) -> Vec<TouchInjection> {
        match &mut self.placement {
            Placement::Wheel { point } => {
                let sign = match direction {
                    ZoomDirection::In => -1.0,
                    ZoomDirection::Out => 1.0,
                };
                point.x = (point.x + sign * plan.wheel_step_dx).clamp(0.0, 1.0);
            }
            Placement::Pinch { half_width, .. } => {
                let factor = match direction {
                    ZoomDirection::In => 1.0 + plan.pinch_step,
                    ZoomDirection::Out => 1.0 - plan.pinch_step,
                };
                *half_width = (*half_width * factor).clamp(MIN_HALF_WIDTH, MAX_HALF_WIDTH);
            }
        }
        self.samples(TouchPhase::Moved, now)
    }

    /// Упёрлись ли пальцы в границу движения в сторону `direction` (дальше двигать некуда).
    pub fn is_at_limit(&self, direction: ZoomDirection) -> bool {
        match (self.placement, direction) {
            (Placement::Wheel { point }, ZoomDirection::In) => point.x <= 0.0,
            (Placement::Wheel { point }, ZoomDirection::Out) => point.x >= 1.0,
            (Placement::Pinch { half_width, .. }, ZoomDirection::In) => half_width >= MAX_HALF_WIDTH,
            (Placement::Pinch { half_width, .. }, ZoomDirection::Out) => half_width <= MIN_HALF_WIDTH,
        }
    }

    /// Образцы всех пальцев в фазе `phase` на текущих местах.
    fn samples(&mut self, phase: TouchPhase, now: MonotonicNanos) -> Vec<TouchInjection> {
        let time = MonotonicNanos::from_nanos(now.as_nanos().max(self.last_time.as_nanos() + 1));
        self.last_time = time;
        self.points()
            .into_iter()
            .enumerate()
            .map(|(index, point)| TouchInjection {
                pointer: PointerId::new(index as i32),
                phase,
                point,
                time,
            })
            .collect()
    }

    /// Точки пальцев: колесо — одна, щипок — левая и правая.
    fn points(&self) -> Vec<ScreenPoint> {
        match self.placement {
            Placement::Wheel { point } => vec![point],
            Placement::Pinch { center, half_width } => vec![
                ScreenPoint::new(center.x - half_width, center.y),
                ScreenPoint::new(center.x + half_width, center.y),
            ],
        }
    }
}

#[cfg(test)]
#[path = "zoom_seamless_fingers_tests.rs"]
mod tests;
