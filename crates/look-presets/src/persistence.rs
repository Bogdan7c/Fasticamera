//! Фоновая запись пресетов: поток рендера диск не ждёт.
//!
//! Схема: «почтовый ящик на одно письмо». `request_save` кладёт последний снимок и будит
//! поток записи; если прежний снимок ещё не взят, он ЗАМЕНЯЕТСЯ (склейка, «последний
//! побеждает», счётчик `coalesced`). Поэтому очередь не растёт и вызов не блокируется
//! (кроме короткой взаимной блокировки на копирование указателя).
//!
//! Завершение: [`PresetPersistence::close`] и `Drop` дописывают последний принятый запрос и
//! ждут поток. Ожидание ограничено временем ОДНОЙ записи на диск; новых запросов поток
//! после закрытия не принимает.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use look_contract::{PresetError, StorageFailure};

use crate::file::{LOG_TARGET, SaveCommit};
use crate::snapshot::LibrarySnapshot;

/// Имя потока записи (видно в профилировщиках и логах).
const WRITER_THREAD_NAME: &str = "fasticamera-look-presets";

/// Куда поток записи пишет снимки. Файл — [`crate::PresetFileStore`]; тесты подставляют заглушку.
pub trait SnapshotSink: Send + 'static {
    /// Записывает снимок целиком (атомарно, если приёмник это умеет).
    fn write_snapshot(&mut self, snapshot: &LibrarySnapshot) -> Result<SaveCommit, PresetError>;

    /// Причина, по которой приёмник вообще не будет писать (например, непрочитанный файл не затираем).
    /// Опрашивается один раз при запуске потока: запросы на запись сразу получают
    /// [`SaveRequestOutcome::Refused`], а не молча ставятся в очередь на заведомый отказ.
    fn save_refusal(&self) -> Option<StorageFailure> {
        None
    }
}

/// Исход одной записи.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveOutcome {
    /// Снимок записан и подтверждён на диске.
    Saved,
    /// Новый файл уже на месте (данные не потеряны), но сброс каталога на диск не подтверждён:
    /// при потере питания в этот миг мог бы вернуться прежний файл. Не ошибка потери данных.
    SavedDirectoryUnconfirmed(StorageFailure),
    /// Запись не удалась (прежний файл цел); экран в 14b покажет ошибку.
    Failed(PresetError),
}

/// Что стало с запросом на запись.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveRequestOutcome {
    /// Принят в пустой ящик.
    Queued,
    /// Принят вместо прежнего ещё не записанного снимка.
    ReplacedPending,
    /// Поток записи закрыт: запрос отброшен.
    Closed,
    /// Поток записи умер (паника приёмника): запрос отброшен, дальнейших записей не будет.
    WriterDied,
    /// Приёмник отказывается писать (см. [`SnapshotSink::save_refusal`], причина — [`PresetPersistence::refusal`]):
    /// запрос отброшен, снимок не будет записан.
    Refused,
}

/// Счётчики записи с момента запуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PersistenceStats {
    /// Успешных записей.
    pub saved: u64,
    /// Записей, где файл заменён, но каталог на диск не подтверждён (не входят в `saved`).
    pub saved_directory_unconfirmed: u64,
    /// Неудачных записей.
    pub failed: u64,
    /// Запросов, склеенных с более поздним (их снимок не писался).
    pub coalesced: u64,
    /// Запросов, отброшенных после смерти потока записи.
    pub rejected_writer_died: u64,
    /// Запросов, отброшенных, потому что приёмник отказывается писать.
    pub rejected_refused: u64,
    /// Поток записи умер от паники приёмника.
    pub writer_died: bool,
}

/// Ящик на одно письмо.
#[derive(Default)]
struct Mailbox {
    pending: Option<LibrarySnapshot>,
    closed: bool,
}

/// Общее состояние потока записи и вызывающего.
struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    outcomes: Mutex<VecDeque<SaveOutcome>>,
    outcome_capacity: usize,
    saved: AtomicU64,
    saved_directory_unconfirmed: AtomicU64,
    failed: AtomicU64,
    coalesced: AtomicU64,
    rejected_writer_died: AtomicU64,
    rejected_refused: AtomicU64,
    /// Поток записи умер от паники (ставит [`WriterExitGuard`]); после этого запросы не принимаются.
    writer_died: AtomicBool,
    /// Причина отказа приёмника писать; фиксируется при запуске и дальше не меняется.
    refusal: Option<StorageFailure>,
}

impl Shared {
    fn lock_mailbox(&self) -> MutexGuard<'_, Mailbox> {
        // Паника в другом потоке не должна ронять запись: данные ящика остаются целыми.
        self.mailbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn lock_outcomes(&self) -> MutexGuard<'_, VecDeque<SaveOutcome>> {
        self.outcomes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, outcome: SaveOutcome) {
        match &outcome {
            SaveOutcome::Saved => self.saved.fetch_add(1, Ordering::Relaxed),
            SaveOutcome::SavedDirectoryUnconfirmed(failure) => {
                tracing::warn!(target: LOG_TARGET, %failure, "пресеты записаны, но каталог на диск не подтверждён");
                self.saved_directory_unconfirmed.fetch_add(1, Ordering::Relaxed)
            }
            SaveOutcome::Failed(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "не удалось записать пресеты");
                self.failed.fetch_add(1, Ordering::Relaxed)
            }
        };
        let mut queue = self.lock_outcomes();
        if queue.len() >= self.outcome_capacity {
            // Старые исходы вытесняются: счётчики всё равно точны.
            queue.pop_front();
        }
        queue.push_back(outcome);
    }
}

/// Поток записи пресетов.
pub struct PresetPersistence {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl PresetPersistence {
    /// Запускает поток записи. `outcome_capacity` — сколько исходов хранить (≥ 1).
    pub fn spawn(sink: impl SnapshotSink, outcome_capacity: usize) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            wake: Condvar::new(),
            outcomes: Mutex::new(VecDeque::new()),
            outcome_capacity: outcome_capacity.max(1),
            saved: AtomicU64::new(0),
            saved_directory_unconfirmed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            rejected_writer_died: AtomicU64::new(0),
            rejected_refused: AtomicU64::new(0),
            writer_died: AtomicBool::new(false),
            refusal: sink.save_refusal(),
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

    /// Просит записать снимок. Не блокирует на диске: кладёт снимок в ящик и будит поток.
    pub fn request_save(&self, snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        // Мёртвый поток проверяем ПЕРВЫМ: он же помечает ящик закрытым, но причина отличается.
        if self.shared.writer_died.load(Ordering::Acquire) {
            self.shared.rejected_writer_died.fetch_add(1, Ordering::Relaxed);
            return SaveRequestOutcome::WriterDied;
        }
        if self.shared.refusal.is_some() {
            self.shared.rejected_refused.fetch_add(1, Ordering::Relaxed);
            return SaveRequestOutcome::Refused;
        }
        let mut mailbox = self.shared.lock_mailbox();
        if mailbox.closed {
            return SaveRequestOutcome::Closed;
        }
        let replaced = mailbox.pending.replace(snapshot).is_some();
        drop(mailbox);
        self.shared.wake.notify_one();
        if replaced {
            self.shared.coalesced.fetch_add(1, Ordering::Relaxed);
            SaveRequestOutcome::ReplacedPending
        } else {
            SaveRequestOutcome::Queued
        }
    }

    /// Почему приёмник отказывается писать (`None` — пишет).
    pub fn refusal(&self) -> Option<&StorageFailure> {
        self.shared.refusal.as_ref()
    }

    /// Забирает накопленные исходы записей (старые первыми).
    pub fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        self.shared.lock_outcomes().drain(..).collect()
    }

    /// Счётчики на данный момент.
    pub fn stats(&self) -> PersistenceStats {
        PersistenceStats {
            saved: self.shared.saved.load(Ordering::Relaxed),
            saved_directory_unconfirmed: self.shared.saved_directory_unconfirmed.load(Ordering::Relaxed),
            failed: self.shared.failed.load(Ordering::Relaxed),
            coalesced: self.shared.coalesced.load(Ordering::Relaxed),
            rejected_writer_died: self.shared.rejected_writer_died.load(Ordering::Relaxed),
            rejected_refused: self.shared.rejected_refused.load(Ordering::Relaxed),
            writer_died: self.shared.writer_died.load(Ordering::Acquire),
        }
    }

    /// Закрывает приём запросов, дожидается записи последнего принятого снимка и потока.
    /// Повторный вызов ничего не делает.
    pub fn close(&mut self) -> PersistenceStats {
        self.shared.lock_mailbox().closed = true;
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::warn!(target: LOG_TARGET, "поток записи пресетов завершился паникой");
        }
        self.stats()
    }
}

impl Drop for PresetPersistence {
    fn drop(&mut self) {
        self.close();
    }
}

/// Страж выхода потока записи: срабатывает при любом выходе, в том числе при панике приёмника.
///
/// Без стража паника молча убивала поток, а `request_save` продолжал отвечать `Queued`: снимки
/// копились в ящике, который уже никто не читает. Страж при панике поднимает `writer_died`,
/// закрывает ящик, выбрасывает недописанный снимок и будит ожидающих.
struct WriterExitGuard<'shared> {
    shared: &'shared Shared,
}

impl Drop for WriterExitGuard<'_> {
    fn drop(&mut self) {
        if !thread::panicking() {
            return;
        }
        // Сначала флаг (его читает `request_save` без блокировки), затем ящик.
        self.shared.writer_died.store(true, Ordering::Release);
        tracing::error!(target: LOG_TARGET, "поток записи пресетов умер от паники приёмника: записи остановлены");
        let mut mailbox = self.shared.lock_mailbox();
        mailbox.closed = true;
        mailbox.pending = None;
        drop(mailbox);
        self.shared.wake.notify_all();
    }
}

/// Тело потока: ждёт снимок, пишет, повторяет; после закрытия дописывает последний и выходит.
fn run_writer(shared: &Shared, mut sink: impl SnapshotSink) {
    let _exit_guard = WriterExitGuard { shared };
    loop {
        let snapshot = {
            let mut mailbox = shared.lock_mailbox();
            loop {
                if let Some(snapshot) = mailbox.pending.take() {
                    break Some(snapshot);
                }
                if mailbox.closed {
                    break None;
                }
                mailbox = shared.wake.wait(mailbox).unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Some(snapshot) = snapshot else { return };
        // Диск пишем БЕЗ блокировки ящика: новые запросы за это время склеиваются.
        let outcome = match sink.write_snapshot(&snapshot) {
            Ok(SaveCommit::Durable) => SaveOutcome::Saved,
            Ok(SaveCommit::ReplacedDirectoryUnconfirmed(failure)) => SaveOutcome::SavedDirectoryUnconfirmed(failure),
            Err(error) => SaveOutcome::Failed(error),
        };
        shared.record(outcome);
    }
}
