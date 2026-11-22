//! Набор снимков, которые уже захвачены, но ещё сохраняются (решение 140: до 10 одновременно).
//!
//! Владеет двумя вещами: какие номера ждут `CaptureFinished` и до какого момента ждать каждого.
//! Затвор о наборе знает только вопрос «есть ли место» ([`SavingShots::has_room`]); сроки, поиск и
//! вытеснение скрыты здесь. Набор ограничен: переполнение — явная ошибка [`SavingSetFull`], а не рост
//! без предела и не молчаливое вытеснение (уже снятое не теряется).

use shell_contract::clock::MonotonicNanos;

use crate::shutter::ShotRequestId;
use crate::time::has_reached;

/// Один сохраняемый снимок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SavingShot {
    request: ShotRequestId,
    /// После этого момента ждать итог бессмысленно: снимок считается потерянным.
    deadline: MonotonicNanos,
}

/// Набор уже захваченных снимков, ждущих сохранения. Порядок — порядок захвата.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavingShots {
    capacity: usize,
    shots: Vec<SavingShot>,
}

/// В наборе нет места: ёмкость исчерпана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SavingSetFull;

impl SavingShots {
    /// Пустой набор. Ёмкость уже проверена конфигом (≥ 1); `max(1)` — страховка от нулевой.
    pub(crate) fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            shots: Vec::with_capacity(capacity),
        }
    }

    /// Есть ли место ещё для одного снимка.
    pub(crate) fn has_room(&self) -> bool {
        self.shots.len() < self.capacity
    }

    /// Сколько снимков сохраняется сейчас.
    pub(crate) fn len(&self) -> usize {
        self.shots.len()
    }

    /// Берёт снимок на сохранение. Нет места — `Err`, набор не меняется.
    pub(crate) fn begin(&mut self, request: ShotRequestId, deadline: MonotonicNanos) -> Result<(), SavingSetFull> {
        if !self.has_room() {
            return Err(SavingSetFull);
        }
        self.shots.push(SavingShot { request, deadline });
        Ok(())
    }

    /// Снимок сохранён или не получился: больше не ждём. `false` — такого снимка в наборе не было.
    pub(crate) fn finish(&mut self, request: ShotRequestId) -> bool {
        let before = self.shots.len();
        self.shots.retain(|shot| shot.request != request);
        self.shots.len() != before
    }

    /// Убирает снимки, чей срок к `now` вышел; возвращает, сколько их было.
    pub(crate) fn expire(&mut self, now: MonotonicNanos) -> usize {
        let before = self.shots.len();
        self.shots.retain(|shot| !has_reached(now, shot.deadline));
        before - self.shots.len()
    }
}
