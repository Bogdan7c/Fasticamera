//! Журнал итогов снимков для диагностики и smoke-сценария `capture`: что и КОГДА сообщила служба записи.
//!
//! # Зачем
//! Экран получает от службы только «кадр захвачен» и «сохранено/ошибка» без адресов файлов и без времени, а
//! сценарию `capture` нужно знать, КАКИЕ файлы появились у каждого снимка и за сколько после нажатия. Журнал
//! пишет таблица «снимок → экран» ([`crate::still_screen_ledger::StillScreenLedger`]) в момент фазы; читает его
//! диагностика ([`crate::capture_backend::CaptureMonitor::phases_after`]).
//!
//! # Инварианты
//! * Только чтение извне и только запись таблицы: журнал не влияет на доставку фаз экрану.
//! * Ограничен: переполнение вытесняет самые старые записи, а читатель ВИДИТ потерю ([`PhaseBatch::lost_entries`]).
//! * Номера записей сквозные и растут на 1: курсор читателя — «последний виденный номер».
//! * Время — [`Instant`] момента записи (часы процесса): сценарий сравнивает его со своим же `Instant` нажатия.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use camera_screen::shutter::ShotRequestId;
use serde::Serialize;
use still_contract::phase::StillPhase;
use still_contract::plan::StillId;

/// Сколько фаз у одного снимка в худшем случае (захвачен, затем сохранён или сорван): запас под вытеснение.
pub const PHASES_PER_STILL: usize = 2;

/// Курсор читателя: номер последней записи, которую он уже видел (0 — не видел ничего).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PhaseCursor(pub u64);

/// Что произошло со снимком.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum PhaseKind {
    /// Кадр захвачен.
    Secured,
    /// Файлы записаны: адрес основной картинки и (если был в плане) DNG.
    Saved { primary: String, raw: Option<String> },
    /// Снимок сорвался; текст — причина службы (`StillFailure`).
    Failed { failure: String },
}

/// Одна запись журнала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseEntry {
    /// Сквозной номер записи (с 1).
    pub seq: u64,
    pub still: StillId,
    /// Номер запроса ЭКРАНА, которому принадлежит снимок.
    pub request: ShotRequestId,
    /// Когда служба сообщила фазу.
    pub at: Instant,
    pub kind: PhaseKind,
}

/// Прочитанная часть журнала.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PhaseBatch {
    /// Записи после курсора, по порядку.
    pub entries: Vec<PhaseEntry>,
    /// Сколько записей после курсора вытеснено переполнением до чтения (0 — читатель видит всё).
    pub lost_entries: u64,
}

impl PhaseBatch {
    /// Курсор после этой порции (прежний, если порция пуста).
    pub fn cursor_after(&self, previous: PhaseCursor) -> PhaseCursor {
        self.entries.last().map_or(previous, |entry| PhaseCursor(entry.seq))
    }
}

#[derive(Default)]
struct LogState {
    /// Номер последней записанной записи.
    last_seq: u64,
    entries: VecDeque<PhaseEntry>,
}

/// Ограниченный журнал фаз.
pub struct StillPhaseLog {
    capacity: usize,
    state: Mutex<LogState>,
}

impl StillPhaseLog {
    /// Журнал на `capacity` записей (не меньше 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, LogState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Записывает фазу снимка. Замок короткий: вызывается с рабочего потока службы записи.
    pub fn record(&self, request: ShotRequestId, phase: &StillPhase) {
        let (still, kind) = match phase {
            StillPhase::Secured { id } => (*id, PhaseKind::Secured),
            StillPhase::Saved { id, files } => (
                *id,
                PhaseKind::Saved {
                    primary: files.primary.clone(),
                    raw: files.raw.clone(),
                },
            ),
            StillPhase::Failed { id, failure } => (
                *id,
                PhaseKind::Failed {
                    failure: failure.to_string(),
                },
            ),
        };
        let mut state = self.lock();
        state.last_seq += 1;
        let entry = PhaseEntry {
            seq: state.last_seq,
            still,
            request,
            at: Instant::now(),
            kind,
        };
        if state.entries.len() >= self.capacity {
            state.entries.pop_front();
        }
        state.entries.push_back(entry);
    }

    /// Курсор «сейчас»: всё, что записано до этого вызова, читателю не нужно.
    pub fn cursor_now(&self) -> PhaseCursor {
        PhaseCursor(self.lock().last_seq)
    }

    /// Записи после курсора. Если часть вытеснена, говорит сколько (читатель обязан это заметить).
    pub fn entries_after(&self, cursor: PhaseCursor) -> PhaseBatch {
        let state = self.lock();
        let entries: Vec<PhaseEntry> = state
            .entries
            .iter()
            .filter(|entry| entry.seq > cursor.0)
            .cloned()
            .collect();
        let first_kept_seq = state.entries.front().map_or(state.last_seq + 1, |entry| entry.seq);
        // Читателю положены номера cursor+1..=last_seq; вытеснены те из них, что младше самой старой хранимой.
        let lost_entries = first_kept_seq.saturating_sub(cursor.0 + 1);
        PhaseBatch { entries, lost_entries }
    }
}

#[cfg(test)]
#[path = "still_phase_log_tests.rs"]
mod tests;
