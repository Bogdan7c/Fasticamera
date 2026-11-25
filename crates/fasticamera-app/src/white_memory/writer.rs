//! Фоновая запись памяти белой: поток движка диск не ждёт.
//!
//! Схема «ящик на одно письмо» (как у пресетов вида): `save` кладёт последнюю запись и будит поток записи.
//! Если прежняя запись ещё не взята, она ЗАМЕНЯЕТСЯ (склейка, «последняя побеждает»): очередь не растёт, вызов
//! не блокируется на диске. В ящике два независимых места — белая и правило возобновления: склейка идёт в каждом
//! отдельно (новая белая не вытесняет выбор правила и наоборот), а частота и «пиши немедленно» у них общие. Если к
//! пробуждению заняты оба места, на диск уходит ОДНА запись с обоими. `Drop` закрывает приём, дописывает последнюю принятую запись и ждёт поток: ожидание
//! ограничено временем ОДНОЙ записи.
//!
//! Частота записи: шкала белой под пальцем присылает правки десятки раз в секунду, а каждая запись на диск —
//! временный файл, переименование и сброс каталога (износ флеш-памяти, лишняя нагрузка). Поэтому поток пишет
//! **не чаще одного раза за `min_write_interval`** (от начала прошлой записи). Принятое значение не теряется:
//! оно ложится не позже, чем через интервал после прошлой записи (первая запись и запись после паузы — сразу).
//! Интервал не действует в двух случаях, когда ждать нельзя: [`WhiteMemory::flush`] (приложение уходит в фон,
//! процесс могут убить) и закрытие (`Drop`).
//!
//! Ошибки записи не молчат: строка в журнал с целью `fasticamera::white` и счётчики + текст последней ошибки
//! в [`WhiteMemoryDiagnostics`] (читается без остановки потока).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use camera_core::white::{WhiteMemory, WhiteMemoryRecord, WhiteResumeRule};

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

/// Что поток записи взял из ящика за одно пробуждение: занятое место или оба.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WriteJob {
    Record(WhiteMemoryRecord),
    ResumeRule(WhiteResumeRule),
    Both {
        record: WhiteMemoryRecord,
        rule: WhiteResumeRule,
    },
}

/// Ящик на одно письмо: два независимых места.
#[derive(Default)]
struct Mailbox {
    pending: Option<WhiteMemoryRecord>,
    /// Выбор правила возобновления, ещё не взятый потоком записи.
    pending_rule: Option<WhiteResumeRule>,
    closed: bool,
    /// Движок велел «пиши немедленно» ([`WhiteMemory::flush`]): принятую запись положить без ожидания интервала.
    /// Взводится только при наличии записи и сбрасывается, когда её взяли.
    flush_requested: bool,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    wake: Condvar,
    diagnostics: WhiteMemoryDiagnostics,
}

impl Mailbox {
    /// Есть ли что писать (хотя бы одно занятое место).
    fn has_pending(&self) -> bool {
        self.pending.is_some() || self.pending_rule.is_some()
    }

    /// Забирает всё занятое одним заданием; `None` — ящик пуст.
    fn take_job(&mut self) -> Option<WriteJob> {
        match (self.pending.take(), self.pending_rule.take()) {
            (Some(record), Some(rule)) => Some(WriteJob::Both { record, rule }),
            (Some(record), None) => Some(WriteJob::Record(record)),
            (None, Some(rule)) => Some(WriteJob::ResumeRule(rule)),
            (None, None) => None,
        }
    }
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
    ///
    /// `min_write_interval` — не чаще одной записи на диск за этот срок (конфиг `camera.white_memory.min_write_interval_ms`).
    pub fn spawn(
        sink: impl WhiteRecordSink,
        diagnostics: WhiteMemoryDiagnostics,
        min_write_interval: Duration,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            wake: Condvar::new(),
            diagnostics,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name(WRITER_THREAD_NAME.to_owned())
            .spawn(move || run_writer(&worker_shared, sink, min_write_interval))?;
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
        self.deposit(|mailbox| mailbox.pending.replace(record).is_some());
    }

    /// То же для выбора правила возобновления: своё место в ящике, склейка только с прежним выбором.
    fn save_resume_rule(&mut self, rule: WhiteResumeRule) {
        self.deposit(|mailbox| mailbox.pending_rule.replace(rule).is_some());
    }

    /// Не ждёт диска: только снимает отсрочку; запись делает поток записи. Без принятой записи — ничего.
    fn flush(&mut self) {
        let mut mailbox = self.shared.lock_mailbox();
        if mailbox.has_pending() && !mailbox.closed {
            mailbox.flush_requested = true;
            drop(mailbox);
            self.shared.wake.notify_one();
        }
    }
}

impl WhiteMemoryWriter {
    /// Общий путь `save*`: учёт запроса, отказ мёртвому или закрытому писателю, кладёт запись в своё место ящика
    /// (`place` возвращает «вытеснила ли она прежнюю») и будит поток.
    fn deposit(&self, place: impl FnOnce(&mut Mailbox) -> bool) {
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
        let replaced = place(&mut mailbox);
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
        mailbox.pending_rule = None;
        drop(mailbox);
        self.shared.wake.notify_all();
    }
}

/// Тело потока: ждёт запись и срок, пишет, повторяет; после закрытия дописывает последнюю и выходит.
fn run_writer(shared: &Shared, mut sink: impl WhiteRecordSink, min_write_interval: Duration) {
    let _exit_guard = WriterExitGuard { shared };
    let mut last_write_started: Option<Instant> = None;
    while let Some(job) = wait_for_job(shared, min_write_interval, last_write_started) {
        last_write_started = Some(Instant::now());
        // Диск пишем БЕЗ блокировки ящика: новые записи за это время склеиваются.
        let outcome = match &job {
            WriteJob::Record(record) => sink.write_record(record),
            WriteJob::ResumeRule(rule) => sink.write_resume_rule(*rule),
            WriteJob::Both { record, rule } => sink.write_record_and_rule(record, *rule),
        };
        report_outcome(&shared.diagnostics, &job, outcome);
    }
}

/// Сколько ещё нельзя писать: интервал считается от начала прошлой записи; записи не было — ждать нечего.
fn remaining_wait(last_write_started: Option<Instant>, min_write_interval: Duration, now: Instant) -> Duration {
    last_write_started.map_or(Duration::ZERO, |started| {
        min_write_interval.saturating_sub(now.saturating_duration_since(started))
    })
}

/// Ждёт, пока появится задание, которое уже можно писать: срок прошёл, либо велено «немедленно», либо ящик закрыт.
/// `None` — ящик закрыт и записей нет, поток выходит.
fn wait_for_job(
    shared: &Shared,
    min_write_interval: Duration,
    last_write_started: Option<Instant>,
) -> Option<WriteJob> {
    let mut mailbox = shared.lock_mailbox();
    loop {
        if !mailbox.has_pending() {
            if mailbox.closed {
                return None;
            }
            mailbox = shared.wake.wait(mailbox).unwrap_or_else(PoisonError::into_inner);
            continue;
        }
        let may_write_now = mailbox.closed || mailbox.flush_requested;
        let wait = if may_write_now {
            Duration::ZERO
        } else {
            remaining_wait(last_write_started, min_write_interval, Instant::now())
        };
        if wait.is_zero() {
            mailbox.flush_requested = false;
            return mailbox.take_job();
        }
        // Новая запись только заменит ожидающую и разбудит нас раньше срока: тогда считаем остаток заново.
        mailbox = shared
            .wake
            .wait_timeout(mailbox, wait)
            .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
    }
}

/// Итог записи: журнал и счётчики.
fn report_outcome(
    diagnostics: &WhiteMemoryDiagnostics,
    job: &WriteJob,
    outcome: Result<WriteCommit, WhiteMemoryError>,
) {
    let cells = &*diagnostics.shared;
    if outcome.is_ok() {
        log_job_written(job);
    }
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

/// Значение, которое реально легло на диск: `info`, потому что записей мало (не чаще интервала). Именно эта строка
/// несёт итог серии ручных правок: строки движка о самих правках идут в `debug`.
fn log_job_written(job: &WriteJob) {
    match job {
        WriteJob::Record(record) | WriteJob::Both { record, .. } => log_written(record),
        WriteJob::ResumeRule(_) => {}
    }
    if let WriteJob::ResumeRule(rule) | WriteJob::Both { rule, .. } = job {
        tracing::info!(target: LOG_TARGET, rule = ?rule, "правило возобновления белого записано в память");
    }
}

pub(super) fn log_written(record: &WhiteMemoryRecord) {
    tracing::info!(
        target: LOG_TARGET,
        kelvin = record.setting.temperature.kelvin(),
        duv = record.setting.tint.duv(),
        origin = ?record.origin,
        "память белой записана"
    );
}
