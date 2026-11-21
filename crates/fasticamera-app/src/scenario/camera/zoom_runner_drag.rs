//! Протяжка пальцем по ленте в исполнителе `zoom` (шаг `wheel_drag`, сессия 13): образцы касаний ровным шагом по
//! времени. Вынесено дочерним модулем из `zoom_runner.rs` (размер файла), поведение не менялось.

use std::time::{Duration, Instant};

use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, TouchPhase};

use super::{ActiveStep, Injection, TouchInjection, ZoomObservation, ZoomRunner};
use crate::screen_scenario_config::ScreenPoint;

/// Протяжка пальцем: сколько образцов отправлено.
pub(super) struct DragProgress {
    /// Сколько образцов `Moved` уже отправлено.
    pub(super) moved: u32,
    /// Отправлен ли `Ended`.
    pub(super) ended: bool,
    /// Когда следующий образец (часы маршрутизатора).
    pub(super) next_at: Instant,
}

impl ZoomRunner {
    pub(super) fn continue_drag(
        &self,
        active: &mut ActiveStep,
        observation: &ZoomObservation<'_>,
        out: &mut Vec<Injection>,
    ) {
        let plan = self.config.wheel_drag;
        let Some(drag) = active.drag.as_mut() else {
            return;
        };
        let interval = Duration::from_millis(plan.sample_interval_ms);
        while !drag.ended && observation.now >= drag.next_at {
            let step_index = drag.moved + 1;
            // Метки образцов идут ровным шагом от момента действия: скорость пальца не зависит от дрожания тиков.
            let time = MonotonicNanos::from_nanos(
                active.action_render.as_nanos()
                    + i64::from(step_index) * i64::try_from(plan.sample_interval_ms).unwrap_or(i64::MAX) * 1_000_000,
            );
            let point = ScreenPoint::new(plan.start.x + plan.step_dx * f64::from(step_index), plan.start.y);
            if drag.moved < plan.samples {
                drag.moved += 1;
                out.push(Injection::Touch(TouchInjection {
                    pointer: PointerId::new(0),
                    phase: TouchPhase::Moved,
                    point,
                    time,
                }));
            } else {
                drag.ended = true;
                out.push(Injection::Touch(TouchInjection {
                    pointer: PointerId::new(0),
                    phase: TouchPhase::Ended,
                    point: ScreenPoint::new(plan.start.x + plan.step_dx * f64::from(plan.samples), plan.start.y),
                    time,
                }));
            }
            drag.next_at += interval;
        }
    }
}
