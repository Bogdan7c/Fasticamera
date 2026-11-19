//! Тап: одиночный или двойной (два тапа подряд, переживает отрыв пальца между ними).

use shell_contract::touch::PointerId;

use super::{Input, StepContext, Verdict};
use crate::geometry::Point;
use crate::gesture::spec::{TapSpec, millis};
use crate::gesture::state::GestureData;
use crate::time::{MonotonicNanos, UiDuration};

#[derive(Debug, Clone)]
pub(in crate::gesture) struct TapState {
    spec: TapSpec,
    /// Палец текущего касания; чужие пальцы (режим `Ignore`) не интересны.
    tracked: Option<PointerId>,
    /// Точка текущего касания (для проверки сдвига).
    touch_down_at: Option<Point>,
    /// Точка первого касания серии (для расстояния между тапами).
    first_touch_at: Option<Point>,
    /// Сколько тапов уже сложилось (палец лёг и оторвался без сдвига).
    taps_done: u8,
    last_lift_time: Option<MonotonicNanos>,
    /// Нужное число тапов набрано, `Ready` отдан: дальше вид молчит, ждёт решения арены.
    completed: bool,
}

impl TapState {
    pub fn new(spec: TapSpec) -> Self {
        Self {
            spec,
            tracked: None,
            touch_down_at: None,
            first_touch_at: None,
            taps_done: 0,
            last_lift_time: None,
            completed: false,
        }
    }

    pub fn spec(&self) -> &TapSpec {
        &self.spec
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.spec.clone());
    }

    /// Первый тап уже был, ждём второй: такой распознаватель живёт между касаниями.
    pub fn awaits_next_tap(&self) -> bool {
        !self.completed && self.taps_done > 0
    }

    /// Промежуток между отрывом и моментом `now` вышел за допустимый (строго больше).
    fn interval_expired(&self, now: MonotonicNanos) -> bool {
        self.last_lift_time
            .is_some_and(|lift| UiDuration::between(lift, now) > millis(self.spec.max_interval_between_taps_ms))
    }

    pub fn step(&mut self, input: Input, context: &StepContext<'_>) -> Verdict {
        if self.completed {
            return Verdict::Idle;
        }
        match input {
            Input::Down(pointer) => self.on_down(pointer, context),
            Input::Move(pointer) => self.on_move(pointer, context),
            Input::Up(pointer) => self.on_up(pointer, context),
            Input::Tick => {
                let waiting_for_second_tap = self.taps_done > 0 && self.touch_down_at.is_none();
                if waiting_for_second_tap && self.interval_expired(context.now) {
                    Verdict::Fail
                } else {
                    Verdict::Idle
                }
            }
        }
    }

    fn on_down(&mut self, pointer: PointerId, context: &StepContext<'_>) -> Verdict {
        if self.tracked.is_some() || context.fingers.len() != 1 {
            return Verdict::Idle;
        }
        let Some(finger) = context.fingers.get(pointer) else {
            return Verdict::Idle;
        };
        if self.taps_done > 0 {
            // Второй тап: вовремя ли и рядом ли с первым.
            let too_far = self
                .first_touch_at
                .is_some_and(|first| first.distance_to(finger.position) > self.spec.max_distance_between_taps_pt);
            if self.interval_expired(context.now) || too_far {
                return Verdict::Fail;
            }
        } else {
            self.first_touch_at = Some(finger.position);
        }
        self.tracked = Some(pointer);
        self.touch_down_at = Some(finger.position);
        Verdict::Idle
    }

    fn on_move(&mut self, pointer: PointerId, context: &StepContext<'_>) -> Verdict {
        if self.tracked != Some(pointer) {
            return Verdict::Idle;
        }
        let (Some(finger), Some(down_at)) = (context.fingers.get(pointer), self.touch_down_at) else {
            return Verdict::Idle;
        };
        if finger.position.distance_to(down_at) > self.spec.max_movement_pt {
            Verdict::Fail
        } else {
            Verdict::Idle
        }
    }

    fn on_up(&mut self, pointer: PointerId, context: &StepContext<'_>) -> Verdict {
        if self.tracked != Some(pointer) {
            return Verdict::Idle;
        }
        let Some(finger) = context.fingers.get(pointer) else {
            return Verdict::Idle;
        };
        self.tracked = None;
        self.touch_down_at = None;
        self.taps_done = self.taps_done.saturating_add(1);
        self.last_lift_time = Some(context.now);
        if self.taps_done >= self.spec.taps.required_taps() {
            self.completed = true;
            Verdict::Ready(GestureData::Tap {
                position: finger.position,
                tap_count: self.taps_done,
            })
        } else {
            Verdict::Idle
        }
    }
}
