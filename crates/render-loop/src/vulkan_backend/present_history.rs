//! Таблица «id показа в Vulkan -> номер кадра потока рендера».
//!
//! `VK_GOOGLE_display_timing` сообщает о показах по `present_id`, который выдаёт `DisplayTarget`
//! (свой счётчик, растёт и на брошенных кадрах). Поток же знает кадры по `frame_index` из
//! `FrameTiming`. Эта таблица — мост между ними. Она ограничена: если записи о показах не
//! приходят (ПК-стенд без display timing), старые соответствия вытесняются, а не копятся.

use std::collections::VecDeque;

/// Ограниченная таблица соответствий.
#[derive(Debug)]
pub(super) struct PresentHistory {
    /// Пары `(present_id, frame_index)` в порядке показа.
    entries: VecDeque<(u32, u64)>,
    capacity: usize,
}

impl PresentHistory {
    /// `capacity == 0` допустим: тогда таблица ничего не помнит (записи display timing
    /// в таком случае просто не получится сопоставить с кадрами).
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.min(1024)),
            capacity,
        }
    }

    /// Запоминает показ; при переполнении вытесняет самое старое соответствие.
    pub(super) fn record(&mut self, present_id: u32, frame_index: u64) {
        if self.capacity == 0 {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((present_id, frame_index));
    }

    /// Находит и УДАЛЯЕТ соответствие: каждая запись display timing приходит один раз.
    /// `None` — id неизвестен (вытеснен или показ чужой).
    pub(super) fn resolve(&mut self, present_id: u32) -> Option<u64> {
        let position = self.entries.iter().position(|&(id, _)| id == present_id)?;
        self.entries.remove(position).map(|(_, frame_index)| frame_index)
    }

    /// Забывает все соответствия. Вызывается при пересоздании цепочки: записи display timing
    /// старой цепочки уже не придут (их вычерпали до пересоздания или они пропали вместе с ней),
    /// и такие соответствия только занимали бы ограниченную таблицу.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_returns_frame_once_and_forgets_it() {
        let mut history = PresentHistory::new(8);
        history.record(5, 100);
        history.record(6, 101);
        assert_eq!(history.resolve(6), Some(101));
        assert_eq!(history.resolve(6), None, "запись выдаётся один раз");
        assert_eq!(history.resolve(5), Some(100));
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn oldest_entries_are_evicted_at_capacity() {
        let mut history = PresentHistory::new(3);
        for present_id in 0..5u32 {
            history.record(present_id, u64::from(present_id) + 10);
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.resolve(0), None);
        assert_eq!(history.resolve(1), None);
        assert_eq!(history.resolve(4), Some(14));
    }

    #[test]
    fn clear_forgets_everything_but_keeps_recording_afterwards() {
        let mut history = PresentHistory::new(4);
        history.record(1, 10);
        history.record(2, 11);
        history.clear();
        assert_eq!(history.len(), 0);
        assert_eq!(history.resolve(1), None, "старое соответствие забыто");
        history.record(3, 12);
        assert_eq!(history.resolve(3), Some(12), "после очистки таблица работает");
    }

    #[test]
    fn zero_capacity_remembers_nothing() {
        let mut history = PresentHistory::new(0);
        history.record(1, 1);
        assert_eq!(history.resolve(1), None);
        assert_eq!(history.len(), 0);
    }
}
