//! Допуск снимков: не больше `max_pending_stills` одновременно ждут сохранения (решение 140).
//!
//! Владелец учёта — [`StillAdmission`]. Слот выдаётся билетом [`AdmissionTicket`] и освобождается
//! РОВНО один раз: когда билет уничтожен. Служба держит билет до итоговой фазы снимка (Saved/Failed),
//! а если приложение взяло билет и не передало службе (например, не ушёл план рендеру), билет
//! освободит слот при выбрасывании. Поэтому «утечка» слота невозможна без утечки самого билета.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use still_contract::phase::StillFailure;
use still_contract::plan::StillId;

/// Почему снимок не допущен. Отказ ничего не трогает: уже снятые снимки идут своим путём.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionRefused {
    /// Ждущих снимков уже `limit` (решение 140).
    #[error("ждущих снимков уже {limit}")]
    QueueFull { limit: usize },
    /// Номер снимка уже в работе: номера сквозные, повтор — ошибка вызывающего.
    #[error("снимок {0:?} уже в работе")]
    DuplicateId(StillId),
    /// Служба остановлена или останавливается.
    #[error("служба снимков остановлена")]
    ServiceStopped,
}

impl From<AdmissionRefused> for StillFailure {
    fn from(refused: AdmissionRefused) -> Self {
        match refused {
            AdmissionRefused::QueueFull { .. } => Self::QueueFull,
            AdmissionRefused::ServiceStopped => Self::ServiceUnavailable,
            AdmissionRefused::DuplicateId(_) => Self::Develop(refused.to_string()),
        }
    }
}

/// Что учёт знает об одном допущенном снимке.
#[derive(Default)]
struct AdmittedStill {
    /// Фаза `Secured` экрану уже отправлена (второй раз её не шлёт никто).
    secured_announced: bool,
}

/// Список снимков в работе и признак «служба закрыта» под одним замком.
#[derive(Default)]
struct AdmissionBook {
    admitted: HashMap<StillId, AdmittedStill>,
    closed: bool,
}

/// Чем кончилась попытка объявить «кадр захвачен».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecuredAnnouncement {
    /// Снимок в работе, фаза отправлена.
    Announced,
    /// Фаза по этому снимку уже отправлялась (повторный `Secured`).
    AlreadyAnnounced,
    /// Снимок не в работе: слот уже освобождён (итог был) или номер не допускался.
    NotPending,
}

/// Владелец учёта допуска.
pub struct StillAdmission {
    limit: usize,
    book: Mutex<AdmissionBook>,
    accepted: AtomicU64,
    refused: AtomicU64,
}

impl StillAdmission {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            book: Mutex::new(AdmissionBook::default()),
            accepted: AtomicU64::new(0),
            refused: AtomicU64::new(0),
        })
    }

    fn book(&self) -> MutexGuard<'_, AdmissionBook> {
        // Замок держат только короткие операции над множеством; отравление не ломает его целостность.
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Выдаёт билет на снимок `id` или отказывает.
    pub fn try_admit(self: &Arc<Self>, id: StillId) -> Result<AdmissionTicket, AdmissionRefused> {
        let verdict = {
            let mut book = self.book();
            if book.closed {
                Err(AdmissionRefused::ServiceStopped)
            } else if book.admitted.contains_key(&id) {
                Err(AdmissionRefused::DuplicateId(id))
            } else if book.admitted.len() >= self.limit {
                Err(AdmissionRefused::QueueFull { limit: self.limit })
            } else {
                book.admitted.insert(id, AdmittedStill::default());
                Ok(())
            }
        };
        match verdict {
            Ok(()) => {
                self.accepted.fetch_add(1, Ordering::Relaxed);
                Ok(AdmissionTicket {
                    id,
                    admission: Arc::clone(self),
                })
            }
            Err(refused) => {
                self.refused.fetch_add(1, Ordering::Relaxed);
                Err(refused)
            }
        }
    }

    /// Объявляет «кадр захвачен»: вызывает `report` ровно один раз за снимок, пока его слот занят.
    ///
    /// `report` выполняется ПОД замком учёта. Это нужно ради порядка: слот освобождается раньше
    /// итоговой фазы (см. `Worker::conclude`), поэтому итог снимка не может обогнать уже начатую
    /// отправку `Secured` — он ждёт замка. Цена: `report` обязан быть коротким и не звать службу
    /// обратно (контракт `StillPhaseSink`).
    pub fn announce_secured(&self, id: StillId, report: impl FnOnce()) -> SecuredAnnouncement {
        let mut book = self.book();
        let Some(entry) = book.admitted.get_mut(&id) else {
            return SecuredAnnouncement::NotPending;
        };
        if entry.secured_announced {
            return SecuredAnnouncement::AlreadyAnnounced;
        }
        entry.secured_announced = true;
        report();
        SecuredAnnouncement::Announced
    }

    /// Закрывает допуск: новые снимки получают [`AdmissionRefused::ServiceStopped`].
    pub fn close(&self) {
        self.book().closed = true;
    }

    /// Сколько снимков сейчас ждут сохранения.
    pub fn pending_count(&self) -> usize {
        self.book().admitted.len()
    }

    /// Сколько снимков допущено за всё время.
    pub fn accepted_total(&self) -> u64 {
        self.accepted.load(Ordering::Relaxed)
    }

    /// Сколько отказов выдано за всё время.
    pub fn refused_total(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    fn release(&self, id: StillId) {
        self.book().admitted.remove(&id);
    }
}

/// Билет на один слот допуска. Слот освобождается при уничтожении билета (ровно один раз).
#[must_use = "билет держит слот допуска; без него снимок не учитывается"]
pub struct AdmissionTicket {
    id: StillId,
    admission: Arc<StillAdmission>,
}

impl AdmissionTicket {
    pub fn still_id(&self) -> StillId {
        self.id
    }
}

impl Drop for AdmissionTicket {
    fn drop(&mut self) {
        self.admission.release(self.id);
    }
}

impl std::fmt::Debug for AdmissionTicket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AdmissionTicket").field("id", &self.id).finish()
    }
}
