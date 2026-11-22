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
//!
//! # Миниатюра (14g, решения 154–156)
//! Рендер отдаёт миниатюру снимка РАНЬШЕ итога ([`StillThumbnailSink::hold_thumbnail`]); таблица держит её
//! при записи своего снимка (не больше одной на снимок) и отдаёт экрану ОДНИМ сообщением вместе с итогом
//! «сохранено» ([`crate::camera_screen_input::ScreenInput::SavedWithPicture`]): картинка без итога или итог
//! без картинки не бывают. Картинка не переживает своего снимка:
//! * `Failed` выбрасывает её со счётчиком;
//! * миниатюра снимка, которого в таблице нет (поздняя, уже закончился, вытеснен, не регистрировался) —
//!   отброшена со счётчиком, вечно не хранится;
//! * вытеснение записи и `unregister` уносят и её картинку;
//! * миниатюра снимка экрана прежней эпохи не хранится (экран сменился, ей некому быть показанной).
//!
//! «Сохранено» без картинки (рендер её не прислал) уходит обычным действием: экран нарисует серый диск.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_screen::ScreenAction;
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId};
use photo_output::service::StillPhaseSink;
use serde::Serialize;
use still_contract::phase::{StillFailure, StillPhase};
use still_contract::plan::StillId;
use still_contract::thumbnail::StillThumbnail;
use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};

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

/// Куда рендер (через форвардер событий) отдаёт миниатюру снимка. Метод не блокирует: короткий замок таблицы.
pub trait StillThumbnailSink: Send + Sync {
    /// Берёт миниатюру на хранение до итога её снимка. Не `bool`: судьба картинки видна в счётчиках приёмника.
    fn hold_thumbnail(&self, thumbnail: StillThumbnail);
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
    /// Миниатюры, взятые на хранение (по одной на снимок).
    pub thumbnails_held: u64,
    /// Миниатюры, ушедшие экрану вместе с итогом «сохранено».
    pub thumbnails_delivered: u64,
    /// Миниатюры, выброшенные итогом `Failed` своего снимка.
    pub thumbnails_dropped_failed: u64,
    /// Поздние миниатюры: снимка нет в таблице (закончился, вытеснен или не регистрировался).
    pub thumbnails_dropped_late: u64,
    /// Миниатюры, унесённые вытеснением записи или `unregister`.
    pub thumbnails_dropped_evicted: u64,
    /// Миниатюры экрана прежней эпохи (экран сменился до миниатюры или до итога).
    pub thumbnails_dropped_stale_screen: u64,
    /// Миниатюры, потерянные вместе с итогом «сохранено» (экрана нет или канал потерял сообщение).
    pub thumbnails_lost_with_phase: u64,
    /// Миниатюры, которые не удалось собрать в картинку UI (размер не сошёлся с буфером).
    pub thumbnails_invalid: u64,
    /// Вторая миниатюра на тот же снимок: заменила первую.
    pub thumbnails_replaced: u64,
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
    thumbnails_held: AtomicU64,
    thumbnails_delivered: AtomicU64,
    thumbnails_dropped_failed: AtomicU64,
    thumbnails_dropped_late: AtomicU64,
    thumbnails_dropped_evicted: AtomicU64,
    thumbnails_dropped_stale_screen: AtomicU64,
    thumbnails_lost_with_phase: AtomicU64,
    thumbnails_invalid: AtomicU64,
    thumbnails_replaced: AtomicU64,
}

/// Под одним замком: владельцы снимков и их ждущие миниатюры (картинка живёт и умирает вместе с записью).
#[derive(Default)]
struct LedgerTable {
    owners: BTreeMap<StillId, Owner>,
    /// Не больше одной картинки на снимок; ключи всегда есть в `owners`.
    pictures: BTreeMap<StillId, UiPicture>,
}

/// Что таблица отдаёт, когда снимает запись итоговой фазой.
struct Finished {
    owner: Owner,
    /// Миниатюра, дожидавшаяся итога.
    picture: Option<UiPicture>,
}

fn bump(cell: &AtomicU64) {
    cell.fetch_add(1, Ordering::Relaxed);
}

/// Таблица «снимок → экран» и приёмник фаз службы записи.
pub struct StillScreenLedger {
    slot: Arc<CaptureCompletionSlot>,
    capacity: usize,
    table: Mutex<LedgerTable>,
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
            table: Mutex::default(),
            counters: CounterCells::default(),
            phase_log: Arc::new(StillPhaseLog::new(capacity.saturating_mul(PHASES_PER_STILL))),
        }
    }

    /// Журнал фаз (только чтение для диагностики; доставку фаз экрану он не меняет).
    pub fn phase_log(&self) -> Arc<StillPhaseLog> {
        Arc::clone(&self.phase_log)
    }

    fn lock(&self) -> MutexGuard<'_, LedgerTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Текущая эпоха экрана: запрос запоминает её в момент постановки.
    pub fn current_screen_epoch(&self) -> u64 {
        self.slot.current_epoch()
    }

    /// Запоминает владельца снимка. Звать ДО передачи билета службе.
    pub fn register(&self, id: StillId, request: ShotRequestId, epoch: u64) -> RegisterOutcome {
        let mut table = self.lock();
        let mut outcome = RegisterOutcome::Registered;
        if table.owners.len() >= self.capacity
            && let Some((&oldest, _)) = table.owners.iter().next()
        {
            table.owners.remove(&oldest);
            self.drop_picture_of(&mut table, oldest, &self.counters.thumbnails_dropped_evicted);
            bump(&self.counters.evicted_registrations);
            tracing::warn!(target: LOG_TARGET, evicted = ?oldest, "таблица снимков полна: вытеснена самая старая запись");
            outcome = RegisterOutcome::RegisteredEvictingOldest { evicted: oldest };
        }
        table.owners.insert(id, Owner { epoch, request });
        outcome
    }

    /// Снимает запись снимка, который служба так и не приняла (фазы не будет). `true` — запись была.
    pub fn unregister(&self, id: StillId) -> bool {
        let mut table = self.lock();
        self.drop_picture_of(&mut table, id, &self.counters.thumbnails_dropped_evicted);
        table.owners.remove(&id).is_some()
    }

    /// Выбрасывает ждущую миниатюру снимка (если есть) и считает её в `reason`.
    fn drop_picture_of(&self, table: &mut LedgerTable, id: StillId, reason: &AtomicU64) {
        if table.pictures.remove(&id).is_some() {
            bump(reason);
        }
    }

    /// Сколько снимков числится за экранами.
    pub fn registered_count(&self) -> usize {
        self.lock().owners.len()
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
            thumbnails_held: load(&self.counters.thumbnails_held),
            thumbnails_delivered: load(&self.counters.thumbnails_delivered),
            thumbnails_dropped_failed: load(&self.counters.thumbnails_dropped_failed),
            thumbnails_dropped_late: load(&self.counters.thumbnails_dropped_late),
            thumbnails_dropped_evicted: load(&self.counters.thumbnails_dropped_evicted),
            thumbnails_dropped_stale_screen: load(&self.counters.thumbnails_dropped_stale_screen),
            thumbnails_lost_with_phase: load(&self.counters.thumbnails_lost_with_phase),
            thumbnails_invalid: load(&self.counters.thumbnails_invalid),
            thumbnails_replaced: load(&self.counters.thumbnails_replaced),
        }
    }

    /// Находит владельца; итоговая фаза запись снимает (вместе с ждущей миниатюрой), `Secured` — нет.
    fn finish_of(&self, phase: &StillPhase) -> Option<Finished> {
        let mut table = self.lock();
        match phase {
            StillPhase::Secured { id } => table
                .owners
                .get(id)
                .copied()
                .map(|owner| Finished { owner, picture: None }),
            StillPhase::Saved { id, .. } | StillPhase::Failed { id, .. } => {
                let owner = table.owners.remove(id)?;
                Some(Finished {
                    owner,
                    picture: table.pictures.remove(id),
                })
            }
        }
    }

    /// Доставка итога «сохранено»: с картинкой — одним сообщением, без неё — обычным действием.
    fn deliver_saved(&self, finished: Finished, request: ShotRequestId, action: ScreenAction) -> EpochDelivery {
        match finished.picture {
            Some(picture) => {
                let delivery = self
                    .slot
                    .deliver_saved_with_picture_in_epoch(finished.owner.epoch, request, picture);
                // Картинка ушла только при `Delivered`; в остальных случаях пропала вместе с сообщением.
                let cell = match delivery {
                    EpochDelivery::Delivered => &self.counters.thumbnails_delivered,
                    EpochDelivery::StaleScreen => &self.counters.thumbnails_dropped_stale_screen,
                    EpochDelivery::NoScreen | EpochDelivery::Lost => &self.counters.thumbnails_lost_with_phase,
                };
                bump(cell);
                delivery
            }
            None => self.slot.deliver_in_epoch(finished.owner.epoch, action),
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

impl StillThumbnailSink for StillScreenLedger {
    fn hold_thumbnail(&self, thumbnail: StillThumbnail) {
        let id = thumbnail.id();
        let size = thumbnail.size();
        // Байты переезжают в картинку UI без копии (`into_rgba`), и делается это ДО замка таблицы.
        let picture = match UiPicture::from_rgba8(
            PictureSize::new(size.width, size.height),
            PictureEncoding::Srgb,
            thumbnail.into_rgba(),
        ) {
            Ok(picture) => picture,
            Err(error) => {
                bump(&self.counters.thumbnails_invalid);
                tracing::warn!(target: LOG_TARGET, ?id, %error, "миниатюра снимка не собралась в картинку: отброшена");
                return;
            }
        };
        let mut table = self.lock();
        let Some(owner) = table.owners.get(&id).copied() else {
            bump(&self.counters.thumbnails_dropped_late);
            tracing::warn!(target: LOG_TARGET, ?id, "миниатюра снимка, которого нет в таблице: отброшена");
            return;
        };
        if owner.epoch != self.slot.current_epoch() {
            // Экран сменился: новому экрану чужая миниатюра не нужна, а хранить её до итога бессмысленно.
            bump(&self.counters.thumbnails_dropped_stale_screen);
            return;
        }
        if table.pictures.insert(id, picture).is_some() {
            bump(&self.counters.thumbnails_replaced);
        }
        bump(&self.counters.thumbnails_held);
    }
}

impl StillPhaseSink for StillScreenLedger {
    fn report_phase(&self, phase: StillPhase) {
        let Some(finished) = self.finish_of(&phase) else {
            bump(&self.counters.unknown_still);
            tracing::warn!(target: LOG_TARGET, id = ?phase.id(), "фаза снимка, которого нет в таблице: отброшена");
            return;
        };
        let owner = finished.owner;
        // `Failed` выбрасывает миниатюру своего снимка: проваленный снимок картинки не ставит.
        if matches!(phase, StillPhase::Failed { .. }) && finished.picture.is_some() {
            bump(&self.counters.thumbnails_dropped_failed);
        }
        // Журнал — до доставки: читатель диагностики, увидев действие на экране, уже находит и запись о нём.
        self.phase_log.record(owner.request, &phase);
        let action = screen_action_of(&phase, owner.request);
        let delivery = match phase {
            StillPhase::Saved { .. } => self.deliver_saved(finished, owner.request, action),
            StillPhase::Secured { .. } | StillPhase::Failed { .. } => self.slot.deliver_in_epoch(owner.epoch, action),
        };
        match delivery {
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
