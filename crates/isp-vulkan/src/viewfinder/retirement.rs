//! Отложенное уничтожение: объект, на который ещё могут ссылаться отправленные команды, живёт до
//! тех пор, пока вызывающий не подтвердит завершение более поздней записи.
//!
//! # Модель
//! Каждая запись кадра (`ViewfinderIsp::record`) получает номер [`RecordSerial`] (растёт на 1).
//! Объект, выведенный из обращения во время записи `N` (изображение сменило размер, вариант пайплайна
//! вытеснен из кэша), использовался только записями `< N` и командами вызывающего, отправленными до
//! сабмита записи `N`. Когда вызывающий увидел fence слота, чья последняя запись `≥ N`
//! (`ViewfinderIsp::collect`), все эти команды завершены: сигнал fence охватывает все более ранние
//! операции очереди (у нас одна очередь), и объект можно уничтожить (`VUID-vkDestroyPipeline-00765`,
//! `VUID-vkDestroyImage-01000`).

/// Номер записи кадра (монотонный у одного `ViewfinderIsp`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RecordSerial(pub u64);

/// Очередь объектов, ждущих завершения GPU.
#[derive(Debug)]
pub(crate) struct RetirementQueue<T> {
    /// Объект и номер записи, начиная с которой он больше не используется.
    pending: Vec<(T, RecordSerial)>,
}

impl<T> Default for RetirementQueue<T> {
    fn default() -> Self {
        Self { pending: Vec::new() }
    }
}

impl<T> RetirementQueue<T> {
    /// Выводит объект из обращения во время записи `unused_from` (она его уже не использует).
    pub fn retire(&mut self, item: T, unused_from: RecordSerial) {
        self.pending.push((item, unused_from));
    }

    /// Уничтожает объекты, которые не нужны ни одной незавершённой записи: завершена запись
    /// `completed`, значит, завершено и всё, что отправлено до неё. Возвращает число освобождённых.
    pub fn release_completed(&mut self, completed: RecordSerial) -> usize {
        let before = self.pending.len();
        self.pending.retain(|(_, unused_from)| *unused_from > completed);
        before - self.pending.len()
    }

    /// Сколько объектов ждёт.
    pub fn len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_lives_until_a_record_at_or_after_retirement_completes() {
        let mut queue = RetirementQueue::default();
        queue.retire("старое изображение", RecordSerial(5));
        queue.retire("старый пайплайн", RecordSerial(7));
        assert_eq!(queue.release_completed(RecordSerial(4)), 0, "запись 4 могла читать оба");
        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue.release_completed(RecordSerial(5)),
            1,
            "запись 5 завершена — изображение свободно"
        );
        assert_eq!(queue.release_completed(RecordSerial(6)), 0);
        assert_eq!(queue.release_completed(RecordSerial(9)), 1);
        assert_eq!(queue.len(), 0);
    }
}
