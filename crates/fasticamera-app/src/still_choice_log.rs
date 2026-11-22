//! Журнал «какой кадр выбрал рендер» для диагностики и smoke-сценария `capture`.
//!
//! # Зачем
//! Выбор кадра ([`FrameChoice`]) живёт только в событии рендера `DevelopEvent::Secured`: служба записи и экран его
//! не получают. Сценарию `capture` нужно доказать «снимок без задержки затвора» (кадр был на экране, начало его
//! экспозиции не далеко от нажатия), поэтому форвардер событий кладёт выбор сюда, прежде чем передать событие службе.
//!
//! # Инварианты
//! * Запись — со ПОТОКА РЕНДЕРА: замок короткий, внутри ни ввода-вывода, ни аллокаций сверх одного элемента очереди.
//! * Журнал ограничен: переполнение вытесняет самые старые записи (читатель видит потерю как «выбор неизвестен»).
//! * Только запись форвардером и только чтение снаружи: журнал не влияет на доставку событий службе.
//! * Ключ — [`StillId`]; повторная запись того же снимка (не бывает по протоколу рендера) заменяет прежнюю.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use still_contract::frame_choice::FrameChoice;
use still_contract::plan::StillId;

/// Сколько выборов хранить по умолчанию. `[estimate]` 64: серия сценария — 10 нажатий, сценарий читает журнал на
/// каждом тике (50 мс), так что запас > 6× от максимума, который может накопиться между чтениями.
pub const DEFAULT_CHOICE_LOG_CAPACITY: usize = 64;

/// Выбор кадра одного снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceEntry {
    pub still: StillId,
    pub choice: FrameChoice,
}

/// Ограниченный журнал выборов кадра.
pub struct StillChoiceLog {
    capacity: usize,
    entries: Mutex<VecDeque<ChoiceEntry>>,
}

impl StillChoiceLog {
    /// Журнал на `capacity` записей (не меньше 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<ChoiceEntry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Запоминает выбор снимка. Вызывается с потока рендера: замок короткий.
    pub fn record(&self, still: StillId, choice: FrameChoice) {
        let mut entries = self.lock();
        entries.retain(|entry| entry.still != still);
        if entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(ChoiceEntry { still, choice });
    }

    /// Все хранимые выборы, старые первыми. Читатель сопоставляет их со снимками по [`StillId`].
    pub fn snapshot(&self) -> Vec<ChoiceEntry> {
        self.lock().iter().copied().collect()
    }

    /// Выбор одного снимка; `None` — не записан (снимок не дошёл до захвата) или уже вытеснен.
    pub fn choice_of(&self, still: StillId) -> Option<FrameChoice> {
        self.lock()
            .iter()
            .find(|entry| entry.still == still)
            .map(|entry| entry.choice)
    }
}

#[cfg(test)]
#[path = "still_choice_log_tests.rs"]
mod tests;
