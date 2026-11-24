//! Учёт тиков, пропущенных как «ничего не изменилось» (решение 214, сессия 16a), для пейсинга. Владелец — `PacingTracker`.
//!
//! Пейсинг судит ровность показа по интервалу между СОСЕДНИМИ номерами показа. Когда экран 120 Гц держит
//! неизменную картинку, показы идут через тик: интервал между соседними показами — два периода, и без поправки
//! пейсинг счёл бы каждый такой интервал пропуском кадра (больше 1,5 периода). Но это задуманная пауза, а не
//! потеря: тик пропущен нарочно.
//!
//! Поток рендера сообщает пейсингу и о пропущенном тике (исход бэкенда), и о показанном кадре, а запись о реальном
//! показе приходит позже (драйвер отдаёт времена с задержкой): поэтому для каждого показанного кадра запоминается,
//! сколько пропусков стояло перед ним, и при приходе записи это число становится поправкой интервала.

use std::collections::VecDeque;

/// Сколько тиков, пропущенных нарочно, стояло перед показанным кадром.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentedFrameSkips {
    /// Номер кадра потока, показанного после пропусков.
    frame_index: u64,
    /// Пропущенные нарочно тики между предыдущим показанным кадром и этим.
    skipped_before: u32,
}

/// Книга пропущенных тиков для пейсинга.
#[derive(Debug)]
pub(super) struct UnchangedSkipLedger {
    /// Пропущенные тики после последнего показанного кадра (ещё не привязаны к показу).
    pending_skips: u32,
    /// Показанные кадры в порядке показа, ждущие записи о реальном показе.
    presented: VecDeque<PresentedFrameSkips>,
    /// Не больше стольких записей (как история показов потока): записи, которые драйвер потерял, не копятся.
    capacity: usize,
}

impl UnchangedSkipLedger {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            pending_skips: 0,
            presented: VecDeque::new(),
            capacity,
        }
    }

    /// Тик пропущен нарочно (бэкенд вернул `SkippedUnchanged`).
    pub(super) fn note_skipped_tick(&mut self) {
        self.pending_skips = self.pending_skips.saturating_add(1);
    }

    /// Кадр `frame_index` показан: пропуски перед ним привязываются к его показу, счёт начинается заново.
    pub(super) fn note_presented_frame(&mut self, frame_index: u64) {
        let skipped_before = std::mem::take(&mut self.pending_skips);
        if self.capacity == 0 {
            return;
        }
        if self.presented.len() == self.capacity {
            self.presented.pop_front();
        }
        self.presented.push_back(PresentedFrameSkips {
            frame_index,
            skipped_before,
        });
    }

    /// Разрыв непрерывности (пересоздание цепочки, отсоединение): номера показов начинаются заново, пропуски до
    /// разрыва к показам после него не относятся.
    pub(super) fn note_discontinuity(&mut self) {
        self.pending_skips = 0;
        self.presented.clear();
    }

    /// Сколько пропусков стояло перед показом кадра `frame_index` (0 — кадр неизвестен: запись вытеснена или кадр
    /// не из этой цепочки). Записи об этом и более ранних кадрах забываются: записи о показах идут по возрастанию.
    pub(super) fn skips_before_presentation(&mut self, frame_index: u64) -> u32 {
        let mut skipped_before = 0;
        while let Some(front) = self.presented.front().copied() {
            if front.frame_index > frame_index {
                break;
            }
            self.presented.pop_front();
            if front.frame_index == frame_index {
                skipped_before = front.skipped_before;
            }
        }
        skipped_before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_before_a_frame_are_attached_to_that_frame_only() {
        let mut ledger = UnchangedSkipLedger::new(16);
        ledger.note_presented_frame(0);
        ledger.note_skipped_tick();
        ledger.note_skipped_tick();
        ledger.note_presented_frame(3);
        ledger.note_presented_frame(4);
        assert_eq!(
            ledger.skips_before_presentation(0),
            0,
            "первому кадру пропуски не предшествовали"
        );
        assert_eq!(
            ledger.skips_before_presentation(3),
            2,
            "два пропуска стояли перед кадром 3"
        );
        assert_eq!(
            ledger.skips_before_presentation(4),
            0,
            "счёт начинается заново после показа"
        );
    }

    #[test]
    fn unknown_or_evicted_frames_have_no_allowance_and_do_not_disturb_later_ones() {
        let mut ledger = UnchangedSkipLedger::new(2);
        ledger.note_skipped_tick();
        ledger.note_presented_frame(1);
        ledger.note_skipped_tick();
        ledger.note_presented_frame(3);
        ledger.note_skipped_tick();
        ledger.note_skipped_tick();
        ledger.note_presented_frame(6);
        // Кадр 1 вытеснен ёмкостью: поправки нет.
        assert_eq!(ledger.skips_before_presentation(1), 0);
        // Запись о кадре 2 (его не было) стирает более ранние, но не трогает более поздние.
        assert_eq!(ledger.skips_before_presentation(2), 0);
        assert_eq!(ledger.skips_before_presentation(3), 1);
        assert_eq!(ledger.skips_before_presentation(6), 2);
    }

    #[test]
    fn discontinuity_forgets_pending_skips_and_waiting_frames() {
        let mut ledger = UnchangedSkipLedger::new(16);
        ledger.note_skipped_tick();
        ledger.note_presented_frame(1);
        ledger.note_skipped_tick();
        ledger.note_discontinuity();
        ledger.note_presented_frame(5);
        assert_eq!(ledger.skips_before_presentation(1), 0, "кадр до разрыва забыт");
        assert_eq!(
            ledger.skips_before_presentation(5),
            0,
            "пропуск до разрыва к новой цепочке не относится"
        );
    }

    #[test]
    fn zero_capacity_keeps_nothing_but_still_resets_the_pending_count() {
        let mut ledger = UnchangedSkipLedger::new(0);
        ledger.note_skipped_tick();
        ledger.note_presented_frame(1);
        ledger.note_presented_frame(2);
        assert_eq!(ledger.skips_before_presentation(1), 0);
        assert_eq!(ledger.skips_before_presentation(2), 0);
    }
}
