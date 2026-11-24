//! Фоновая запись памяти белой: поток движка диск не ждёт.
//!
//! Схема «ящик на одно письмо» (как у пресетов вида): `save` кладёт последнюю запись и будит поток записи.
//! Если прежняя запись ещё не взята, она ЗАМЕНЯЕТСЯ (склейка, «последняя побеждает»): очередь не растёт, вызов
//! не блокируется на диске. `Drop` закрывает приём, дописывает последнюю принятую запись и ждёт поток: ожидание
//! ограничено временем ОДНОЙ записи.
//!
//! Ошибки записи не молчат: строка в журнал с целью `fasticamera::white` и счётчики + текст последней ошибки
//! в [`WhiteMemoryDiagnostics`] (читается без остановки потока).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use camera_core::white::{WhiteMemory, WhiteMemoryRecord};

use super::file::{LOG_TARGET, WhiteMemoryError, WhiteRecordSink, WriteCommit};

/// Имя потока записи (видно в профилировщиках и логах).
const WRITER_THREAD_NAME: &str = "fasticamera-white-memory";

/// Счётчики и последняя ошибка записи на данный момент.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhiteMemoryStats {
    /// Принято в ящик (включая склеенные).
    pub requested: u64,
    /// Записей, подтверждённых на диске.
    pub saved: u64,
    /// Записей, где файл заменён, но каталог не сброшен на диск.
    pub saved_directory_unconfirmed: u64,
    /// Неудачных записей.
    pub failed: u64,
    /// Записей, вытесненных более поздней до того, как поток их взял.
    pub coalesced: u64,
    /// Записей, отброшенных после закрытия или смерти потока.
    pub rejected: u64,
    /// Поток записи умер от паники.
    pub writer_died: bool,
    /// Текст последней ошибки записи (`None` — ошибок не было или после неё запись удалась).
    pub last_error: Option<String>,
}

/// Общие со стороны диагностики счётчики. Клоны делят состояние.
#[derive(Debug, Clone, Default)]
pub struct WhiteMemoryDiagnostics {
    shared: Arc<DiagnosticsCells>,
}

#[derive(Debug, Default)]
struct DiagnosticsCells {
    requested: AtomicU64,
    saved: AtomicU64,
    saved_directory_unconfirmed: AtomicU64,
    failed: AtomicU64,
    coalesced: AtomicU64,
    rejected: AtomicU64,
    writer_died: AtomicBool,
    last_error: Mutex<Option<String>>,
}

impl WhiteMemoryDiagnostics {
    pub fn snapshot(&self) -> WhiteMemoryStats {
        let cells = &*self.shared;
        WhiteMemoryStats {
            requested: cells.requested.load(Ordering::Relaxed),
            saved: cells.saved.load(Ordering::Relaxed),
            saved_directory_unconfirmed: cells.saved_directory_unconfirmed.load(Ordering::Relaxed),
            failed: cells.failed.load(Ordering::Relaxed),
            coalesced: cells.coalesced.load(Ordering::Relaxed),
            rejected: cells.rejected.load(Ordering::Relaxed),
            writer_died: cells.writer_died.load(Ordering::Acquire),
            last_error: cells.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone(),
        }
    }

    fn set_last_error(&self, value: Option<String>) {
        *self.shared.last_error.lock().unwrap_or_else(PoisonError::into_inner) = value;
    }
}

/// Ящик на одно письмо.
#[derive(Default)]
struct Mailbox {
    pending: Option<WhiteMemoryRecord>,
    closed: bool,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    diagnostics: WhiteMemoryDiagnostics,
}

impl Shared {
    fn lock_mailbox(&self) -> MutexGuard<'_, Mailbox> {
        // Паника в другом потоке не должна ронять запись: данные ящика остаются целыми.
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Запись памяти белой на своём потоке. Реализует границу движка [`WhiteMemory`].
pub struct WhiteMemoryWriter {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl WhiteMemoryWriter {
    /// Запускает поток записи. `Err` — ОС не дала поток: вызывающий работает без сохранения и пишет это в журнал.
    pub fn spawn(sink: impl WhiteRecordSink, diagnostics: WhiteMemoryDiagnostics) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            wake: Condvar::new(),
            diagnostics,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name(WRITER_THREAD_NAME.to_owned())
            .spawn(move || run_writer(&worker_shared, sink))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Закрывает приём, дописывает последнюю принятую запись и ждёт поток. Повторный вызов ничего не делает.
    pub fn close(&mut self) -> WhiteMemoryStats {
        self.shared.lock_mailbox().closed = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!(target: LOG_TARGET, "поток записи памяти белой завершился паникой");
        }
        self.shared.diagnostics.snapshot()
    }
}

impl WhiteMemory for WhiteMemoryWriter {
    /// Не блокирует на диске: кладёт запись в ящик и будит поток.
    fn save(&mut self, record: WhiteMemoryRecord) {
        let cells = &*self.shared.diagnostics.shared;
        cells.requested.fetch_add(1, Ordering::Relaxed);
        // Мёртвый поток проверяем первым: писать некому.
        if cells.writer_died.load(Ordering::Acquire) {
            cells.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut mailbox = self.shared.lock_mailbox();
        if mailbox.closed {
            drop(mailbox);
            cells.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let replaced = mailbox.pending.replace(record).is_some();
        drop(mailbox);
        self.shared.wake.notify_one();
        if replaced {
            cells.coalesced.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for WhiteMemoryWriter {
    fn drop(&mut self) {
        self.close();
    }
}

/// Страж выхода потока записи: при панике приёмника поднимает `writer_died` и закрывает ящик, чтобы запросы
/// не копились там, откуда их уже никто не возьмёт.
struct WriterExitGuard<'shared> {
    shared: &'shared Shared,
}

impl Drop for WriterExitGuard<'_> {
    fn drop(&mut self) {
        if !thread::panicking() {
            return;
        }
        self.shared
            .diagnostics
            .shared
            .writer_died
            .store(true, Ordering::Release);
        tracing::error!(target: LOG_TARGET, "поток записи памяти белой умер от паники приёмника: записи остановлены");
        let mut mailbox = self.shared.lock_mailbox();
        mailbox.closed = true;
        mailbox.pending = None;
        drop(mailbox);
        self.shared.wake.notify_all();
    }
}

/// Тело потока: ждёт запись, пишет, повторяет; после закрытия дописывает последнюю и выходит.
fn run_writer(shared: &Shared, mut sink: impl WhiteRecordSink) {
    let _exit_guard = WriterExitGuard { shared };
    loop {
        let record = {
            let mut mailbox = shared.lock_mailbox();
            loop {
                if let Some(record) = mailbox.pending.take() {
                    break Some(record);
                }
                if mailbox.closed {
                    break None;
                }
                mailbox = shared.wake.wait(mailbox).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Some(record) = record else { return };
        // Диск пишем БЕЗ блокировки ящика: новые записи за это время склеиваются.
        report_outcome(&shared.diagnostics, sink.write_record(&record));
    }
}

/// Итог записи: журнал и счётчики.
fn report_outcome(diagnostics: &WhiteMemoryDiagnostics, outcome: Result<WriteCommit, WhiteMemoryError>) {
    let cells = &*diagnostics.shared;
    match outcome {
        Ok(WriteCommit::Durable) => {
            cells.saved.fetch_add(1, Ordering::Relaxed);
            diagnostics.set_last_error(None);
        }
        Ok(WriteCommit::DirectoryUnconfirmed(failure)) => {
            tracing::warn!(target: LOG_TARGET, %failure, "память белой записана, но каталог на диск не подтверждён");
            cells.saved_directory_unconfirmed.fetch_add(1, Ordering::Relaxed);
            diagnostics.set_last_error(None);
        }
        Err(error) => {
            tracing::warn!(target: LOG_TARGET, %error, "не удалось записать память белой");
            cells.failed.fetch_add(1, Ordering::Relaxed);
            diagnostics.set_last_error(Some(error.to_string()));
        }
    }
}
