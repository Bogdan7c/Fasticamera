//! Входная очередь службы: команды приложения и события рендера в одном порядке.
//!
//! Одна очередь нужна ради порядка: билет допуска всегда приходит раньше событий своего снимка.
//! Рендер не должен ждать: события считаются, и при переполнении лимита `submit` сразу возвращает
//! ошибку вместе с событием (рендер отпустит его ресурсы), а не блокируется. Билеты и остановка
//! лимитом не ограничены: билетов не больше `max_pending_stills`, остановка одна.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Mutex, PoisonError};

use still_contract::develop::DevelopEvent;

use super::admission::AdmissionTicket;

/// Команда рабочему потоку.
pub(super) enum Command {
    /// Снимок допущен: служба берёт билет и начинает ждать кадр.
    BeginStill(AdmissionTicket),
    /// Событие проявления от рендера.
    Develop(DevelopEvent),
    /// Остановиться: незаконченные снимки срываются.
    Shutdown,
}

/// Почему команда не принята. Отвергнутая команда уничтожается: ресурсы события (поверхность,
/// картинка) отпускаются их `Drop`, билет освобождает слот допуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InboxRejection {
    /// Очередь событий полна.
    Full,
    /// Рабочий поток уже завершён.
    Stopped,
}

/// Передатчик в очередь службы со счётом событий.
pub(super) struct Inbox {
    sender: Sender<Command>,
    queued_events: AtomicUsize,
    event_capacity: usize,
    /// `true` после [`Inbox::close_for_good`]. Замок держится на время отправки: так «закрыто» и
    /// «команда уже в канале» не могут разойтись (билет не проскочит между последней чисткой и
    /// закрытием).
    closed: Mutex<bool>,
}

impl Inbox {
    pub(super) fn new(sender: Sender<Command>, event_capacity: usize) -> Self {
        Self {
            sender,
            queued_events: AtomicUsize::new(0),
            event_capacity,
            closed: Mutex::new(false),
        }
    }

    /// Ставит событие рендера в очередь, не блокируясь.
    pub(super) fn push_event(&self, event: DevelopEvent) -> Result<(), InboxRejection> {
        let closed = self.lock_closed();
        if *closed {
            return Err(InboxRejection::Stopped);
        }
        // Занимаем место заранее: два потока не пройдут лимит одновременно.
        let previous = self.queued_events.fetch_add(1, Ordering::AcqRel);
        if previous >= self.event_capacity {
            self.queued_events.fetch_sub(1, Ordering::AcqRel);
            return Err(InboxRejection::Full);
        }
        self.sender.send(Command::Develop(event)).map_err(|_| {
            self.queued_events.fetch_sub(1, Ordering::AcqRel);
            InboxRejection::Stopped
        })
    }

    /// Рабочий поток вынул событие из очереди: место освобождается.
    pub(super) fn mark_event_taken(&self) {
        self.queued_events.fetch_sub(1, Ordering::AcqRel);
    }

    /// Передаёт службе билет допуска. При остановленной службе билет уничтожается, слот освобождается.
    pub(super) fn push_ticket(&self, ticket: AdmissionTicket) -> Result<(), InboxRejection> {
        let closed = self.lock_closed();
        if *closed {
            return Err(InboxRejection::Stopped);
        }
        self.sender
            .send(Command::BeginStill(ticket))
            .map_err(|_| InboxRejection::Stopped)
    }

    /// Просит рабочий поток остановиться. `false` — поток уже завершён.
    pub(super) fn push_shutdown(&self) -> bool {
        let closed = self.lock_closed();
        !*closed && self.sender.send(Command::Shutdown).is_ok()
    }

    /// Закрывает очередь навсегда и возвращает всё, что в ней успело лечь. После возврата ни одна
    /// команда в канал не попадёт, поэтому каждому вернувшемуся билету/событию можно дать честный итог.
    pub(super) fn close_for_good(&self, commands: &Receiver<Command>) -> Vec<Command> {
        *self.lock_closed() = true;
        let mut leftovers = Vec::new();
        loop {
            match commands.try_recv() {
                Ok(command) => leftovers.push(command),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return leftovers,
            }
        }
    }

    fn lock_closed(&self) -> std::sync::MutexGuard<'_, bool> {
        // Замок защищает один флаг; отравление его не портит.
        self.closed.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
