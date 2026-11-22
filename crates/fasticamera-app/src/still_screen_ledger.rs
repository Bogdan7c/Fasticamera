//! Учёт «чей это снимок»: номер снимка службы → экран и номер запроса экрана. Приёмник фаз службы записи.
//!
//! # Зачем
//! Номер снимка (`StillId`) сквозной на процесс и принадлежит службе, а экран знает только свой номер запроса
//! (`ShotRequestId`), который у каждого нового экрана начинается с 1 заново. Фазы службы идут с её рабочего
//! потока и не знают ни экрана, ни его номеров. Эта таблица склеивает их: при постановке запроса запоминает
//! `(эпоха экрана, номер запроса)`, при фазе переводит её в действие экрану и доставляет в канал ТОЙ ЖЕ эпохи.
//!
//! # Инварианты
//! * Запись ставится ДО передачи билета службе (иначе фаза с рабочего потока опередила бы запись).
//! * `Secured` запись не снимает, `Saved`/`Failed` снимают: ровно один итог на снимок.
//! * Фаза экрана, которого уже нет (эпоха сменилась), не доставляется новому экрану и считается.
//! * Таблица ограничена; переполнение вытесняет самую старую запись со счётчиком, а не растёт молча.
//! * Фаза не блокирует: берёт короткий замок таблицы и кладёт действие в ограниченный канал экрана.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_screen::ScreenAction;
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId};
use photo_output::service::StillPhaseSink;
use serde::Serialize;
use still_contract::phase::{StillFailure, StillPhase};
use still_contract::plan::StillId;

use crate::camera_screen_capture::{CaptureCompletionSlot, EpochDelivery};
use crate::still_phase_log::{PHASES_PER_STILL, StillPhaseLog};

const LOG_TARGET: &str = "fasticamera::still::ledger";

/// Как причина срыва службы выглядит для экрана (только для диагностики и хаптики: экран отличает три причины).
///
/// Исчерпывающий `match` без `_`: новая причина службы обязана получить осознанное отображение.
pub fn capture_failure_reason(failure: &StillFailure) -> CaptureFailureReason {
    match failure {
        // Камеры, кадра или рендера нет — «сбой камеры» с точки зрения человека.
        StillFailure::ServiceUnavailable | StillFailure::NoFrame(_) => CaptureFailureReason::Camera,
        // Очередь снимков полна — обработка не успевает; ISP/GPU и кодер — обработка.
        StillFailure::QueueFull | StillFailure::Develop(_) | StillFailure::Encode(_) => {
            CaptureFailureReason::Processing
        }
        // Файл не записан (MediaStore, DNG).
        StillFailure::Store(_) => CaptureFailureReason::Storage,
    }
}

/// Кому принадлежит снимок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owner {
    /// Эпоха экрана на момент запроса (`CaptureCompletionSlot::current_epoch`).
    epoch: u64,
    request: ShotRequestId,
}

/// Итог постановки записи в таблицу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered,
    /// Таблица была полна: вытеснена самая старая запись (её фазы теперь «неизвестный снимок»).
    RegisteredEvictingOldest {
        evicted: StillId,
    },
}

/// Счётчики таблицы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct LedgerCounters {
    pub secured_delivered: u64,
    pub saved_delivered: u64,
    pub failed_delivered: u64,
    /// Фазы, чей экран сменился: снимок обещан старому экрану, новому его фаза не нужна.
    pub dropped_stale_screen: u64,
    /// Фазы, которым не нашлось экрана (снят на время сценария).
    pub dropped_no_screen: u64,
    /// Фазы, потерянные каналом (переполнение или уничтоженный экран).
    pub lost_in_channel: u64,
    /// Фазы снимка, которого в таблице нет (вытеснен, не регистрировался или итог уже был).
    pub unknown_still: u64,
    /// Записи, вытесненные при переполнении таблицы.
    pub evicted_registrations: u64,
}

#[derive(Default)]
struct CounterCells {
    secured_delivered: AtomicU64,
    saved_delivered: AtomicU64,
    failed_delivered: AtomicU64,
    dropped_stale_screen: AtomicU64,
    dropped_no_screen: AtomicU64,
    lost_in_channel: AtomicU64,
    unknown_still: AtomicU64,
    evicted_registrations: AtomicU64,
}

fn bump(cell: &AtomicU64) {
    cell.fetch_add(1, Ordering::Relaxed);
}

/// Таблица «снимок → экран» и приёмник фаз службы записи.
pub struct StillScreenLedger {
    slot: Arc<CaptureCompletionSlot>,
    capacity: usize,
    owners: Mutex<BTreeMap<StillId, Owner>>,
    counters: CounterCells,
    /// Журнал фаз для диагностики (адреса файлов и время): пишется здесь же, где фаза узнаёт свой экран.
    phase_log: Arc<StillPhaseLog>,
}

impl StillScreenLedger {
    /// Таблица на `capacity` записей (не меньше 1) с ячейкой адреса текущего экрана.
    pub fn new(slot: Arc<CaptureCompletionSlot>, capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slot,
            capacity,
            owners: Mutex::default(),
            counters: CounterCells::default(),
            phase_log: Arc::new(StillPhaseLog::new(capacity.saturating_mul(PHASES_PER_STILL))),
        }
    }

    /// Журнал фаз (только чтение для диагностики; доставку фаз экрану он не меняет).
    pub fn phase_log(&self) -> Arc<StillPhaseLog> {
        Arc::clone(&self.phase_log)
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<StillId, Owner>> {
        self.owners.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Текущая эпоха экрана: запрос запоминает её в момент постановки.
    pub fn current_screen_epoch(&self) -> u64 {
        self.slot.current_epoch()
    }

    /// Запоминает владельца снимка. Звать ДО передачи билета службе.
    pub fn register(&self, id: StillId, request: ShotRequestId, epoch: u64) -> RegisterOutcome {
        let mut owners = self.lock();
        let mut outcome = RegisterOutcome::Registered;
        if owners.len() >= self.capacity
            && let Some((&oldest, _)) = owners.iter().next()
        {
            owners.remove(&oldest);
            bump(&self.counters.evicted_registrations);
            tracing::warn!(target: LOG_TARGET, evicted = ?oldest, "таблица снимков полна: вытеснена самая старая запись");
            outcome = RegisterOutcome::RegisteredEvictingOldest { evicted: oldest };
        }
        owners.insert(id, Owner { epoch, request });
        outcome
    }

    /// Снимает запись снимка, который служба так и не приняла (фазы не будет). `true` — запись была.
    pub fn unregister(&self, id: StillId) -> bool {
        self.lock().remove(&id).is_some()
    }

    /// Сколько снимков числится за экранами.
    pub fn registered_count(&self) -> usize {
        self.lock().len()
    }

    pub fn counters(&self) -> LedgerCounters {
        let load = |cell: &AtomicU64| cell.load(Ordering::Relaxed);
        LedgerCounters {
            secured_delivered: load(&self.counters.secured_delivered),
            saved_delivered: load(&self.counters.saved_delivered),
            failed_delivered: load(&self.counters.failed_delivered),
            dropped_stale_screen: load(&self.counters.dropped_stale_screen),
            dropped_no_screen: load(&self.counters.dropped_no_screen),
            lost_in_channel: load(&self.counters.lost_in_channel),
            unknown_still: load(&self.counters.unknown_still),
            evicted_registrations: load(&self.counters.evicted_registrations),
        }
    }

    /// Находит владельца; итоговая фаза запись снимает, `Secured` — нет.
    fn owner_of(&self, phase: &StillPhase) -> Option<Owner> {
        let mut owners = self.lock();
        match phase {
            StillPhase::Secured { id } => owners.get(id).copied(),
            StillPhase::Saved { id, .. } | StillPhase::Failed { id, .. } => owners.remove(id),
        }
    }
}

/// Действие экрану и «чей счётчик растёт, когда оно доставлено».
fn screen_action_of(phase: &StillPhase, request: ShotRequestId) -> ScreenAction {
    match phase {
        StillPhase::Secured { .. } => ScreenAction::CaptureSecured { request },
        StillPhase::Saved { .. } => ScreenAction::CaptureFinished {
            request,
            outcome: CaptureOutcome::Saved,
        },
        StillPhase::Failed { failure, .. } => ScreenAction::CaptureFinished {
            request,
            outcome: CaptureOutcome::Failed(capture_failure_reason(failure)),
        },
    }
}

impl StillPhaseSink for StillScreenLedger {
    fn report_phase(&self, phase: StillPhase) {
        let Some(owner) = self.owner_of(&phase) else {
            bump(&self.counters.unknown_still);
            tracing::warn!(target: LOG_TARGET, id = ?phase.id(), "фаза снимка, которого нет в таблице: отброшена");
            return;
        };
        // Журнал — до доставки: читатель диагностики, увидев действие на экране, уже находит и запись о нём.
        self.phase_log.record(owner.request, &phase);
        let action = screen_action_of(&phase, owner.request);
        match self.slot.deliver_in_epoch(owner.epoch, action) {
            EpochDelivery::Delivered => {
                let cell = match phase {
                    StillPhase::Secured { .. } => &self.counters.secured_delivered,
                    StillPhase::Saved { .. } => &self.counters.saved_delivered,
                    StillPhase::Failed { .. } => &self.counters.failed_delivered,
                };
                bump(cell);
            }
            EpochDelivery::StaleScreen => {
                bump(&self.counters.dropped_stale_screen);
                tracing::warn!(target: LOG_TARGET, id = ?phase.id(), "экран сменился: фаза снимка старого экрана снята");
            }
            EpochDelivery::NoScreen => bump(&self.counters.dropped_no_screen),
            EpochDelivery::Lost => {
                bump(&self.counters.lost_in_channel);
                tracing::warn!(target: LOG_TARGET, id = ?phase.id(), "канал экрана потерял фазу снимка");
            }
        }
    }
}

#[cfg(test)]
#[path = "still_screen_ledger_tests.rs"]
mod tests;
