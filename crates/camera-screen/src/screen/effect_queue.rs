//! Ограниченная очередь эффектов, которую опустошает приложение (`drain_effects`).

use crate::effect::Effect;

/// Очередь с предельной ёмкостью. Переполнение — не молчаливая потеря: новый эффект отбрасывается и считается.
#[derive(Debug)]
pub(crate) struct EffectQueue {
    effects: Vec<Effect>,
    capacity: usize,
    queued: u64,
    overflowed: u64,
}

/// Итог постановки эффекта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PushOutcome {
    Queued,
    /// Очередь полна: эффект отброшен.
    Overflowed,
}

impl EffectQueue {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            effects: Vec::new(),
            capacity,
            queued: 0,
            overflowed: 0,
        }
    }

    /// Эффекты «последнее побеждает»: новое намерение объектива и новое кадрирование заменяют ещё не забранные
    /// прежние (у колеса и щипка они идут на каждом кадре; очередь не должна ни копить их, ни, переполнившись,
    /// потерять именно последнее, итоговое значение).
    fn supersedes(newer: &Effect, pending: &Effect) -> bool {
        matches!(
            (newer, pending),
            (Effect::Lens(_), Effect::Lens(_)) | (Effect::Viewfinder(_), Effect::Viewfinder(_))
        )
    }

    pub(crate) fn push(&mut self, effect: Effect) -> PushOutcome {
        if let Some(pending) = self
            .effects
            .iter_mut()
            .find(|pending| Self::supersedes(&effect, pending))
        {
            *pending = effect;
            self.queued += 1;
            return PushOutcome::Queued;
        }
        if self.effects.len() >= self.capacity {
            self.overflowed += 1;
            return PushOutcome::Overflowed;
        }
        self.effects.push(effect);
        self.queued += 1;
        PushOutcome::Queued
    }

    /// Отдаёт накопленное в порядке постановки. Хранилище уходит целиком (`mem::take`), без поэлементного
    /// копирования; пустая очередь отдаёт `Vec` без выделения памяти (частый случай: опрос каждый кадр).
    pub(crate) fn drain(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    pub(crate) fn queued(&self) -> u64 {
        self.queued
    }

    pub(crate) fn overflowed(&self) -> u64 {
        self.overflowed
    }
}
