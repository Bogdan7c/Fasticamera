//! Отложенное удаление `AImageReader` закрытых сессий (нейтральная часть учёта).
//!
//! # Зачем reader нельзя просто удалить
//! `AImageReader_delete` освобождает **все** выданные образы reader-а. Но кадр, выданный потоку
//! рендера (`RawFrameLease`), — это именно такой образ: GPU может ещё читать его память. Поэтому
//! reader закрытой сессии не удаляется, а *откладывается* ([`RetiredReaders::retire`]) и удаляется
//! только тогда, когда число невозвращённых кадров упало до нуля.
//!
//! # Потоки и замки
//! * Удаляет всегда поток камеры (в [`RetiredReaders::reap`] на каждом вызове бэкенда и при
//!   закрытии устройства); поток камеры **никогда не ждёт** поток рендера, кроме одного
//!   ограниченного ожидания при остановке бэкенда ([`RetiredReaders::shutdown`]).
//! * Возврат кадра (`AImage_deleteAsync`) делает поток рендера и лишь уменьшает счётчик; reader он
//!   не трогает, замок списка не берёт.
//! * Новые кадры из отложенного reader-а не приходят: ворота слушателя закрыты до отсрочки, значит
//!   число невозвращённых кадров может только убывать.
//!
//! Сам список и счётчики нейтральны: reader абстрагирован трейтом [`RetirableReader`], и логика
//! проверяется на ПК с поддельным reader-ом.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::diagnostics::BackendDiagnostics;

const LOG_TARGET: &str = "fasticamera::camera::reader";

/// Reader, который можно отложить и удалить, когда все его кадры возвращены.
pub trait RetirableReader {
    /// Сколько кадров этого reader-а ещё не возвращено (на руках у потребителей).
    fn outstanding_frames(&self) -> u64;

    /// Удаляет reader (на телефоне — `AImageReader_delete`). Вызывается только при нуле
    /// невозвращённых кадров и закрытых воротах слушателя.
    fn delete(self);
}

/// Итог одного прохода [`RetiredReaders::reap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReapReport {
    pub deleted: usize,
    pub still_waiting: usize,
}

/// Итог остановки [`RetiredReaders::shutdown`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    pub deleted: usize,
    /// Reader-ы, кадры которых не вернулись вовремя: оставлены до конца процесса.
    pub leaked: usize,
}

/// Список отложенных reader-ов.
pub struct RetiredReaders<Reader> {
    waiting: Mutex<Vec<Reader>>,
    diagnostics: Arc<BackendDiagnostics>,
}

impl<Reader: RetirableReader> RetiredReaders<Reader> {
    pub fn new(diagnostics: Arc<BackendDiagnostics>) -> Self {
        Self {
            waiting: Mutex::new(Vec::new()),
            diagnostics,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Reader>> {
        // Паника в другом потоке под замком не должна навсегда запереть закрытие reader-ов.
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Откладывает reader закрытой сессии; если кадров на руках уже нет, удаляет сразу.
    pub fn retire(&self, reader: Reader) {
        self.diagnostics.reader().record_reader_retired();
        self.lock().push(reader);
        self.reap();
    }

    /// Удаляет те отложенные reader-ы, у которых не осталось невозвращённых кадров.
    /// Остальные не трогает.
    pub fn reap(&self) -> ReapReport {
        let (ready, still_waiting) = {
            let mut waiting = self.lock();
            let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *waiting)
                .into_iter()
                .partition(|reader| reader.outstanding_frames() == 0);
            *waiting = rest;
            (ready, waiting.len())
        };
        let deleted = ready.len();
        // Удаление идёт вне замка: `AImageReader_delete` ждёт остановки looper-а и может быть долгим.
        for reader in ready {
            reader.delete();
            self.diagnostics.reader().record_reader_deleted();
        }
        ReapReport { deleted, still_waiting }
    }

    /// Сколько reader-ов ждут возврата кадров.
    pub fn waiting_count(&self) -> usize {
        self.lock().len()
    }

    /// Остановка бэкенда: ждёт возврата кадров не дольше `timeout`, затем оставшиеся reader-ы
    /// **намеренно утекают** (счётчик `readers_leaked` и запись в логе): удалить их значило бы
    /// освободить память, которую GPU ещё может читать.
    pub fn shutdown(&self, timeout: Duration, poll_interval: Duration) -> ShutdownReport {
        let deadline = Instant::now() + timeout;
        let mut deleted = self.reap().deleted;
        while self.waiting_count() > 0 && Instant::now() < deadline {
            thread::sleep(poll_interval.min(deadline.saturating_duration_since(Instant::now())));
            deleted += self.reap().deleted;
        }
        let abandoned = std::mem::take(&mut *self.lock());
        let leaked = abandoned.len();
        for reader in abandoned {
            self.diagnostics.reader().record_reader_leaked();
            tracing::error!(
                target: LOG_TARGET,
                outstanding_frames = reader.outstanding_frames(),
                "reader не удалён: кадры не вернулись за отведённое время, reader оставлен до конца процесса"
            );
            std::mem::forget(reader);
        }
        ShutdownReport { deleted, leaked }
    }
}

#[cfg(test)]
#[path = "retired_readers_tests.rs"]
mod tests;
