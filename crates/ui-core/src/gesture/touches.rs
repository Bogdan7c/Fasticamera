//! Пальцы, лежащие на экране: положения, точки касания, история скорости.

use shell_contract::touch::PointerId;

use crate::geometry::Point;
use crate::input::PointerSample;
use crate::time::MonotonicNanos;
use crate::velocity::VelocityTracker;

/// Один палец.
#[derive(Debug, Clone)]
pub(super) struct Finger {
    pub pointer: PointerId,
    /// Точка касания.
    pub start_position: Point,
    pub position: Point,
    pub time: MonotonicNanos,
    pub velocity: VelocityTracker,
}

/// Пальцы в порядке прихода. Первый — «основной» для однопальцевых жестов.
#[derive(Debug, Clone)]
pub(super) struct Fingers {
    /// Пустой оценщик; клонируется для каждого нового пальца (конфиг проверен при создании арены).
    velocity_template: VelocityTracker,
    list: Vec<Finger>,
}

impl Fingers {
    pub fn new(velocity_template: VelocityTracker) -> Self {
        Self {
            velocity_template,
            list: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn contains(&self, pointer: PointerId) -> bool {
        self.get(pointer).is_some()
    }

    pub fn get(&self, pointer: PointerId) -> Option<&Finger> {
        self.list.iter().find(|finger| finger.pointer == pointer)
    }

    /// Первые два пальца (пара щипка) в порядке прихода.
    pub fn first_two(&self) -> Option<(&Finger, &Finger)> {
        match self.list.as_slice() {
            [first, second, ..] => Some((first, second)),
            _ => None,
        }
    }

    /// Новый палец: запоминает точку касания и первую выборку скорости.
    pub fn begin(&mut self, sample: PointerSample) {
        let mut velocity = self.velocity_template.clone();
        velocity.clear();
        velocity.add_sample(sample.time, sample.position);
        self.list.push(Finger {
            pointer: sample.pointer,
            start_position: sample.position,
            position: sample.position,
            time: sample.time,
            velocity,
        });
    }

    /// Палец сдвинулся: положение и выборка скорости.
    pub fn moved(&mut self, sample: PointerSample) {
        if let Some(finger) = self.list.iter_mut().find(|finger| finger.pointer == sample.pointer) {
            finger.position = sample.position;
            finger.time = sample.time;
            finger.velocity.add_sample(sample.time, sample.position);
        }
    }

    /// Палец оторвался: последнее положение обновляется, а выборка скорости **не** добавляется
    /// (как в AOSP: отрыв не несёт информации о движении).
    pub fn lifted(&mut self, sample: PointerSample) {
        if let Some(finger) = self.list.iter_mut().find(|finger| finger.pointer == sample.pointer) {
            finger.position = sample.position;
            finger.time = sample.time;
        }
    }

    pub fn remove(&mut self, pointer: PointerId) {
        self.list.retain(|finger| finger.pointer != pointer);
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }
}
