//! Отношения между распознавателями: «требует провала» и «одновременно».

use super::state::RecognizerId;

/// Ошибка отношения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelationError {
    #[error("распознаватель {0:?} нельзя связать с самим собой")]
    WithItself(RecognizerId),
    #[error("«{waiting:?} требует провала {must_fail:?}» замыкает цикл ожидания")]
    Cycle {
        waiting: RecognizerId,
        must_fail: RecognizerId,
    },
}

/// Хранилище отношений. Порядок вставки сохраняется (`Vec`), чтобы обход был детерминирован.
#[derive(Debug, Default, Clone)]
pub(super) struct Relations {
    /// `(ждущий, тот, чьего провала ждут)`.
    must_fail: Vec<(RecognizerId, RecognizerId)>,
    /// Пары, которым разрешено распознаваться одновременно; внутри пары `min, max`.
    simultaneous: Vec<(RecognizerId, RecognizerId)>,
}

impl Relations {
    /// `waiting` не сработает, пока `must_fail` не провалится. Повтор той же пары — не ошибка.
    pub fn require_to_fail(&mut self, waiting: RecognizerId, must_fail: RecognizerId) -> Result<(), RelationError> {
        if waiting == must_fail {
            return Err(RelationError::WithItself(waiting));
        }
        if self.waits_transitively(must_fail, waiting) {
            return Err(RelationError::Cycle { waiting, must_fail });
        }
        if !self.must_fail.contains(&(waiting, must_fail)) {
            self.must_fail.push((waiting, must_fail));
        }
        Ok(())
    }

    pub fn allow_simultaneous(&mut self, first: RecognizerId, second: RecognizerId) -> Result<(), RelationError> {
        if first == second {
            return Err(RelationError::WithItself(first));
        }
        let pair = (first.min(second), first.max(second));
        if !self.simultaneous.contains(&pair) {
            self.simultaneous.push(pair);
        }
        Ok(())
    }

    /// Идёт ли от `from` цепочка ожиданий до `target`.
    fn waits_transitively(&self, from: RecognizerId, target: RecognizerId) -> bool {
        let mut stack = vec![from];
        let mut seen: Vec<RecognizerId> = Vec::new();
        while let Some(current) = stack.pop() {
            if current == target {
                return true;
            }
            if seen.contains(&current) {
                continue;
            }
            seen.push(current);
            stack.extend(self.blockers_of(current));
        }
        false
    }

    /// Чьего провала ждёт `waiting`.
    pub fn blockers_of(&self, waiting: RecognizerId) -> impl Iterator<Item = RecognizerId> + '_ {
        self.must_fail
            .iter()
            .filter(move |(w, _)| *w == waiting)
            .map(|(_, blocker)| *blocker)
    }

    /// Ждёт ли `waiting` провала `blocker` напрямую.
    pub fn is_waiting_for(&self, waiting: RecognizerId, blocker: RecognizerId) -> bool {
        self.must_fail.contains(&(waiting, blocker))
    }

    pub fn are_simultaneous(&self, first: RecognizerId, second: RecognizerId) -> bool {
        self.simultaneous.contains(&(first.min(second), first.max(second)))
    }

    /// Убрать все отношения с удалённым распознавателем.
    pub fn forget(&mut self, removed: RecognizerId) {
        self.must_fail
            .retain(|(waiting, blocker)| *waiting != removed && *blocker != removed);
        self.simultaneous
            .retain(|(first, second)| *first != removed && *second != removed);
    }
}
