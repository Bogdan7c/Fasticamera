//! Учёт серий сабмитов показа: когда GPU точно закончил работу с ресурсами кадров.
//!
//! Каждый принятый сабмит получает возрастающий номер ([`SubmissionSerial`]). Завершение известно
//! косвенно: слот кольца выдаётся снова только после fence слота (`acquire_frame` его ждёт), а fence
//! сабмита охватывает все более ранние сабмиты очереди (спецификация `vkQueueSubmit`: первая область
//! синхронизации fence включает все команды, раньше отправленные в эту очередь). Значит, получив
//! слот `s`, мы знаем: сабмит, занимавший `s` в прошлый раз, и ВСЕ до него завершены.

use gpu_vulkan::hardware_buffer_cache::SubmissionSerial;

/// Отслеживание серий сабмитов по слотам кольца кадров.
#[derive(Debug)]
pub struct SubmissionTracker {
    /// Серия последнего принятого сабмита (0 — сабмитов не было).
    last_submitted: u64,
    /// Серия, о завершении которой и всех ранних известно наверняка.
    completed: u64,
    /// Серия сабмита, занимавшего слот в последний раз.
    slot_serials: Vec<Option<u64>>,
}

impl SubmissionTracker {
    /// Пустой учёт: сабмитов не было, ничего не завершено.
    pub fn new() -> Self {
        Self {
            last_submitted: 0,
            completed: 0,
            slot_serials: Vec::new(),
        }
    }

    /// Слот выдан кольцом (его fence ждали): всё, что до прошлого сабмита этого слота включительно, завершено.
    /// Возвращает серию, до которой (включительно) всё завершено.
    pub fn note_slot_acquired(&mut self, slot: usize) -> SubmissionSerial {
        if let Some(Some(serial)) = self.slot_serials.get(slot) {
            self.completed = self.completed.max(*serial);
        }
        SubmissionSerial(self.completed)
    }

    /// Серия для сабмита, который сейчас готовится. Она «принята» только после [`Self::note_submitted`].
    pub fn next_serial(&self) -> SubmissionSerial {
        SubmissionSerial(self.last_submitted + 1)
    }

    /// Очередь приняла сабмит этой серии в слоте `slot`.
    pub fn note_submitted(&mut self, slot: usize, serial: SubmissionSerial) {
        self.last_submitted = self.last_submitted.max(serial.0);
        if self.slot_serials.len() <= slot {
            self.slot_serials.resize(slot + 1, None);
        }
        self.slot_serials[slot] = Some(serial.0);
    }

    /// GPU подтверждённо простаивает (дождались все сабмиты цели): завершено всё отправленное.
    pub fn note_gpu_idle(&mut self) {
        self.completed = self.last_submitted;
    }

    /// Серия, до которой всё завершено.
    #[cfg(test)]
    pub fn completed(&self) -> SubmissionSerial {
        SubmissionSerial(self.completed)
    }

    /// Все принятые сабмиты завершены (известно наверняка).
    pub fn everything_completed(&self) -> bool {
        self.completed >= self.last_submitted
    }
}

impl Default for SubmissionTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_is_learned_only_when_the_slot_is_handed_out_again() {
        let mut tracker = SubmissionTracker::new();
        // Два слота кольца: сабмиты 1 (слот 0), 2 (слот 1), 3 (слот 0 снова).
        assert_eq!(
            tracker.note_slot_acquired(0),
            SubmissionSerial(0),
            "ничего не отправлено"
        );
        tracker.note_submitted(0, tracker.next_serial());
        assert_eq!(
            tracker.note_slot_acquired(1),
            SubmissionSerial(0),
            "слот 1 ещё не занимали"
        );
        tracker.note_submitted(1, tracker.next_serial());
        assert!(!tracker.everything_completed());
        assert_eq!(
            tracker.note_slot_acquired(0),
            SubmissionSerial(1),
            "слот 0 свободен: сабмит 1 завершён, сабмит 2 — нет"
        );
        tracker.note_submitted(0, tracker.next_serial());
        assert_eq!(tracker.note_slot_acquired(1), SubmissionSerial(2));
        assert_eq!(tracker.completed(), SubmissionSerial(2));
    }

    #[test]
    fn rejected_submit_does_not_advance_serials() {
        let mut tracker = SubmissionTracker::new();
        let first = tracker.next_serial();
        // Сабмит отвергнут: note_submitted не вызывали — следующая попытка получает ту же серию.
        assert_eq!(tracker.next_serial(), first);
        tracker.note_submitted(0, first);
        assert_eq!(tracker.next_serial(), SubmissionSerial(2));
    }

    #[test]
    fn gpu_idle_marks_everything_submitted_as_completed() {
        let mut tracker = SubmissionTracker::new();
        tracker.note_submitted(0, tracker.next_serial());
        tracker.note_submitted(1, tracker.next_serial());
        assert!(!tracker.everything_completed());
        tracker.note_gpu_idle();
        assert!(tracker.everything_completed());
        assert_eq!(tracker.completed(), SubmissionSerial(2));
    }
}
