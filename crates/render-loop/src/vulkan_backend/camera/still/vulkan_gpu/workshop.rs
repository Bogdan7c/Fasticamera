//! Мастерская снимка (решение 144): фоновый поток для тяжёлой ПОДГОТОВКИ, которой не место на потоке рендера.
//!
//! # Зачем
//! Шаги движка снимков идут на потоке рендера сразу после показа кадра видоискателя: всё, что шаг держит
//! поток, следующий кадр видоискателя ждёт. Замер 14c на телефоне (Adreno 740, release): подключение Vulkan
//! к поверхности кодера HEIF (`EncoderInputTarget::attach`: поверхность + swapchain) — 67–104 мс на снимок,
//! буфер следующей копии RAW (24 МиБ: выделение, отображение, обнуление) — ≈ 28 мс. В серии видоискатель
//! терял ≈ 40 кадров. Сама работа GPU (копия, полосы, показ) стоит единицы мс и остаётся на потоке рендера.
//!
//! # Что можно отдавать в мастерскую
//! Только работу, которая создаёт НОВЫЕ объекты и не трогает чужие: создание поверхности и swapchain на
//! своей поверхности, выделение памяти. Спецификация Vulkan («Threading Behavior»): команды можно звать из
//! разных потоков одновременно, исключение — параметры с внешней синхронизацией (у `vkCreateSwapchainKHR` —
//! `surface` и `oldSwapchain`), а их работа мастерской держит в единоличном владении. Результат возвращается
//! через канал `std::sync::mpsc` — он же даёт нужный барьер памяти между потоками. Очередь GPU мастерская
//! не трогает никогда.
//!
//! # Инварианты
//! * Поток рендера мастерскую не ждёт никогда: [`Ticket::poll`] не блокирует, ожидающего метода нет.
//! * Работы выполняются по одной, в порядке отправки (один поток).
//! * Каждая работа даёт ровно один итог: результат или [`TicketPoll::NoResult`] (работа упала или мастерская
//!   остановлена до неё). Падение одной работы не убивает мастерскую.
//! * Поток рендера мастерскую не ждёт и при остановке: `Drop` закрывает очередь, ещё не начатые работы
//!   пропускаются (их входы уничтожаются, билеты видят `NoResult`), а начатая доделывается на своём потоке без
//!   `join`. Иначе разбор GPU снимков ждал бы подключение цели кодера (до ≈ 0,3 с на телефоне) и мог превысить
//!   бюджет квитанции освобождения surface (`release_receipt_budget_ms`). Устройство Vulkan при этом живо:
//!   работа держит свой клон `GpuContext`.
//! * Результат, чей билет уже брошен, уничтожается на потоке мастерской; для результатов, которые нужно
//!   разобрать особо (цель кодера), — [`StillWorkshop::submit_or_dispose`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

/// Имя потока мастерской (видно в `top -H`, трассах и отчётах о падениях).
const WORKSHOP_THREAD_NAME: &str = "fasticamera-still-workshop";

/// Работа мастерской: замыкание, которое само отправляет свой результат в билет.
type Job = Box<dyn FnOnce() + Send>;

/// Почему работа не принята.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum WorkshopError {
    /// Поток мастерской не запустился.
    #[error("поток мастерской снимка не запустился: {0}")]
    Spawn(String),
    /// Мастерская остановлена (поток завершился): работу некому делать.
    #[error("мастерская снимка остановлена")]
    Stopped,
}

/// Итог неблокирующего опроса билета.
#[derive(Debug)]
pub(super) enum TicketPoll<T> {
    /// Работа ещё не закончена.
    Pending,
    /// Результат работы.
    Ready(T),
    /// Результата не будет: работа упала (паника) или мастерская остановлена до неё.
    NoResult,
}

/// Билет одной работы: по нему забирают результат.
#[derive(Debug)]
pub(super) struct Ticket<T> {
    receiver: Receiver<T>,
}

impl<T> Ticket<T> {
    /// Опрос без ожидания (поток рендера).
    pub(super) fn poll(&self) -> TicketPoll<T> {
        match self.receiver.try_recv() {
            Ok(result) => TicketPoll::Ready(result),
            Err(TryRecvError::Empty) => TicketPoll::Pending,
            // Отправитель уничтожен без отправки: работа упала или не начиналась.
            Err(TryRecvError::Disconnected) => TicketPoll::NoResult,
        }
    }
}

/// Мастерская: один фоновый поток и очередь работ.
pub(super) struct StillWorkshop {
    /// `Option`: `Drop` закрывает очередь (уничтожает отправителя).
    jobs: Option<Sender<Job>>,
    /// Мастерская останавливается: ещё не начатые работы пропускаются.
    stopping: Arc<AtomicBool>,
    /// Поток не присоединяется при остановке (см. модуль): уничтожение ручки отпускает его доделать начатое.
    _thread: JoinHandle<()>,
}

impl std::fmt::Debug for StillWorkshop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StillWorkshop")
            .field("running", &self.jobs.is_some())
            .finish()
    }
}

impl StillWorkshop {
    /// Запускает поток мастерской.
    pub(super) fn start() -> Result<Self, WorkshopError> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread = thread::Builder::new()
            .name(WORKSHOP_THREAD_NAME.to_owned())
            .spawn(move || run_jobs(&inbox, &thread_stopping))
            .map_err(|error| WorkshopError::Spawn(error.to_string()))?;
        Ok(Self {
            jobs: Some(jobs),
            stopping,
            _thread: thread,
        })
    }

    /// Ставит работу в очередь и возвращает её билет. Работа выполнится на потоке мастерской. `label` — имя
    /// работы в логе отладки (моменты начала и конца по `CLOCK_MONOTONIC`: их сверяют с долгими кадрами рендера).
    pub(super) fn submit<T, Work>(&self, label: &'static str, work: Work) -> Result<Ticket<T>, WorkshopError>
    where
        T: Send + 'static,
        Work: FnOnce() -> T + Send + 'static,
    {
        self.submit_or_dispose(label, work, drop)
    }

    /// То же, но результат, чей билет к концу работы уже брошен (остановка, потеря GPU), отдаётся `dispose`
    /// на потоке мастерской: так неиспользованная цель кодера разбирается по правилам, а не бросается в утечку,
    /// и поток рендера её не ждёт.
    pub(super) fn submit_or_dispose<T, Work, Dispose>(
        &self,
        label: &'static str,
        work: Work,
        dispose: Dispose,
    ) -> Result<Ticket<T>, WorkshopError>
    where
        T: Send + 'static,
        Work: FnOnce() -> T + Send + 'static,
        Dispose: FnOnce(T) + Send + 'static,
    {
        let (result_sender, receiver) = mpsc::sync_channel::<T>(1);
        let job: Job = Box::new(move || {
            let started = monotonic_nanos();
            let result = work();
            tracing::debug!(
                target: super::super::LOG_TARGET,
                label,
                started_ns = started,
                finished_ns = monotonic_nanos(),
                "работа мастерской снимка"
            );
            // Билет мог быть уже брошен: результат разбирается здесь, на потоке мастерской.
            if let Err(orphan) = result_sender.send(result) {
                dispose(orphan.0);
            }
        });
        let jobs = self.jobs.as_ref().ok_or(WorkshopError::Stopped)?;
        jobs.send(job).map_err(|_| WorkshopError::Stopped)?;
        Ok(Ticket { receiver })
    }
}

impl Drop for StillWorkshop {
    fn drop(&mut self) {
        // Не начатые работы пропускаются, закрытая очередь завершает цикл. Поток НЕ присоединяется (см. модуль):
        // начатая работа доделывается сама, её результат разберёт `dispose` на потоке мастерской.
        self.stopping.store(true, Ordering::SeqCst);
        self.jobs = None;
    }
}

/// Момент `CLOCK_MONOTONIC` для лога отладки (`None` в логе — часы не прочитались; работе это не мешает).
fn monotonic_nanos() -> Option<i64> {
    use crate::thread::{MonotonicTimer, SystemMonotonicTimer};
    SystemMonotonicTimer.now().ok().map(|now| now.as_nanos())
}

/// Цикл потока: работы по одной, пока очередь не закрыта.
fn run_jobs(inbox: &Receiver<Job>, stopping: &AtomicBool) {
    while let Ok(job) = inbox.recv() {
        if stopping.load(Ordering::SeqCst) {
            // Остановка: работа не начинается, её входы и отправитель уничтожаются (билет увидит `NoResult`).
            drop(job);
            continue;
        }
        // Паника одной работы не должна оставлять остальные без исполнителя: её отправитель уничтожен при
        // раскрутке стека, билет увидит `NoResult`.
        if catch_unwind(AssertUnwindSafe(job)).is_err() {
            tracing::error!(target: super::super::LOG_TARGET, "работа мастерской снимка упала (паника)");
        }
    }
}

#[cfg(test)]
#[path = "workshop_tests.rs"]
mod tests;
