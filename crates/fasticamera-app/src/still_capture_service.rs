//! Настоящий сервис снимка приложения: нажатие экрана → допуск → план рендеру → фазы обратно экрану.
//!
//! # Владельцы
//! * допуск и учёт «сколько снимков ждут» — `photo-output` (`StillAdmission`);
//! * кадр, проявление, цель основной картинки — поток рендера (`render-loop`);
//! * запись файлов и итоговые фазы — служба записи (`photo-output`);
//! * «чей снимок» и эпоха экрана — [`StillScreenLedger`];
//! * ЭТОТ модуль только склеивает: номер снимка, план из запроса, порядок шагов. Логики съёмки в нём нет.
//!
//! # Порядок шагов `submit` (нарушать нельзя)
//! 1. `try_admit` — слот допуска; отказ — ничего не тронуто;
//! 2. запись в таблицу «снимок → экран» — ДО передачи билета, иначе фаза с рабочего потока опередила бы запись;
//! 3. `begin_still` — служба начинает ждать кадр (ДО отправки плана рендеру: иначе событие рендера придёт раньше);
//! 4. `Begin` рендеру. Не ушёл — службе сообщается итоговое событие `Failed`: билет уже у неё, и слот иначе
//!    освободился бы лишь по сроку захвата.
//!
//! # «Ровно один `CaptureFinished` на запрос»
//! Либо `Rejected(..)` (тогда итог экрану даёт исполнитель эффектов, шаги 2–4 не начаты или отменены), либо
//! `Accepted` и ровно одна итоговая фаза службы (в том числе срыв по сроку). Путь «рендер не принял» возвращает
//! `Accepted`: итог придёт фазой `Failed`.
//!
//! Сервис не блокирует поток рендера и главный поток: замки короткие, очереди ограничены.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_screen::shutter::ShotRequest;
use photo_output::service::{AdmissionRefused, StillServiceHandle};
use render_loop::still::{StillBegin, StillCommand};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::plan::{FrameSelection, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};

use crate::camera_screen_capture::{CaptureService, SubmitOutcome, SubmitRejection};
use crate::still_event_forwarder::DevelopEventForwarder;
use crate::still_render_port::{StillCommandDelivery, StillRenderPort};
use crate::still_screen_ledger::StillScreenLedger;

const LOG_TARGET: &str = "fasticamera::still::capture";

/// План снимка из запроса экрана. Всё, что меняется во времени (зум, соотношение, поворот), зафиксировано
/// запросом на момент нажатия, поэтому план его только переводит на язык рендера.
///
/// Вспышка в план пока не входит (камера её не поддерживает, экран шлёт `Off`; 14d).
pub fn plan_from_shot(id: StillId, request: &ShotRequest) -> StillPlan {
    StillPlan {
        id,
        press: ShutterPress { at: request.pressed_at },
        selection: FrameSelection::ShownAtPress,
        framing: request.framing.clone(),
        orientation: request.orientation,
        outputs: StillOutputs {
            primary: request.primary_format,
            raw: match request.raw {
                camera_screen::settings::RawCapture::Off => RawOutput::Skip,
                camera_screen::settings::RawCapture::On => RawOutput::Dng,
            },
        },
    }
}

/// Счётчики сервиса снимка (постановка запросов).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct CaptureCounters {
    /// Запросы, принятые службой и отправленные рендеру (или сорванные при отправке — они тоже приняты).
    pub accepted: u64,
    /// Отказ допуска: очередь ждущих снимков полна.
    pub rejected_queue_full: u64,
    /// Отказ: служба остановлена или не приняла билет.
    pub rejected_service_stopped: u64,
    /// Отказ: номер снимка уже в работе (ошибка нумерации, в норме 0).
    pub rejected_duplicate_id: u64,
    /// Принятые запросы, которых рендер не принял (завершён или не поднимался): снимок сорван фазой `Failed`.
    pub render_refused: u64,
    /// Срыв по `render_refused`, который не удалось сообщить службе (её очередь полна/остановлена):
    /// итог такого снимка даст срок захвата.
    pub abort_event_lost: u64,
}

#[derive(Default)]
struct CounterCells {
    accepted: AtomicU64,
    rejected_queue_full: AtomicU64,
    rejected_service_stopped: AtomicU64,
    rejected_duplicate_id: AtomicU64,
    render_refused: AtomicU64,
    abort_event_lost: AtomicU64,
}

fn bump(cell: &AtomicU64) {
    cell.fetch_add(1, Ordering::Relaxed);
}

/// Что собирает сервис снаружи.
pub struct StillCaptureParts {
    pub service: StillServiceHandle,
    pub render: Arc<dyn StillRenderPort>,
    pub ledger: Arc<StillScreenLedger>,
    pub events: Arc<DevelopEventForwarder>,
    /// Ёмкость журнала принятых запросов (не меньше 1).
    pub journal_capacity: usize,
}

/// Реализация [`CaptureService`] поверх службы записи и потока рендера.
pub struct StillCaptureService {
    service: StillServiceHandle,
    render: Arc<dyn StillRenderPort>,
    ledger: Arc<StillScreenLedger>,
    events: Arc<DevelopEventForwarder>,
    /// Последний выданный номер: номера сквозные на процесс и начинаются с 1.
    last_id: AtomicU64,
    journal: Mutex<VecDeque<ShotRequest>>,
    journal_capacity: usize,
    counters: CounterCells,
}

impl StillCaptureService {
    pub fn new(parts: StillCaptureParts) -> Self {
        Self {
            service: parts.service,
            render: parts.render,
            ledger: parts.ledger,
            events: parts.events,
            last_id: AtomicU64::new(0),
            journal: Mutex::default(),
            journal_capacity: parts.journal_capacity.max(1),
            counters: CounterCells::default(),
        }
    }

    fn next_id(&self) -> StillId {
        StillId(self.last_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn lock_journal(&self) -> MutexGuard<'_, VecDeque<ShotRequest>> {
        self.journal.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Последние принятые запросы по порядку (старые вытеснены).
    pub fn journal(&self) -> Vec<ShotRequest> {
        self.lock_journal().iter().cloned().collect()
    }

    pub fn counters(&self) -> CaptureCounters {
        let load = |cell: &AtomicU64| cell.load(Ordering::Relaxed);
        CaptureCounters {
            accepted: load(&self.counters.accepted),
            rejected_queue_full: load(&self.counters.rejected_queue_full),
            rejected_service_stopped: load(&self.counters.rejected_service_stopped),
            rejected_duplicate_id: load(&self.counters.rejected_duplicate_id),
            render_refused: load(&self.counters.render_refused),
            abort_event_lost: load(&self.counters.abort_event_lost),
        }
    }

    fn record_in_journal(&self, request: &ShotRequest) {
        let mut journal = self.lock_journal();
        if journal.len() >= self.journal_capacity {
            journal.pop_front();
        }
        journal.push_back(request.clone());
    }

    /// Рендер не принял план: сообщаем службе итоговое событие, чтобы она освободила слот и дала экрану `Failed`.
    fn abort_still(&self, id: StillId, delivery: StillCommandDelivery) {
        bump(&self.counters.render_refused);
        let failure = match delivery {
            StillCommandDelivery::RenderGone => DevelopFailure::Abandoned,
            StillCommandDelivery::RenderUnavailable { reason } => {
                DevelopFailure::Gpu(format!("рендер недоступен: {reason}"))
            }
            // Ушло — сюда не попадаем; ветка оставлена исчерпывающей ради новых вариантов шва.
            StillCommandDelivery::Delivered => return,
        };
        tracing::warn!(target: LOG_TARGET, ?id, %failure, "рендер не принял снимок: срываем его через службу");
        if self.events.forward(DevelopEvent::Failed { id, failure }).is_err() {
            bump(&self.counters.abort_event_lost);
        }
    }
}

impl CaptureService for StillCaptureService {
    fn submit(&self, request: ShotRequest, _now: MonotonicNanos) -> SubmitOutcome {
        let id = self.next_id();
        let ticket = match self.service.try_admit(id) {
            Ok(ticket) => ticket,
            Err(AdmissionRefused::QueueFull { limit }) => {
                bump(&self.counters.rejected_queue_full);
                tracing::warn!(target: LOG_TARGET, ?id, limit, "ждущих снимков уже предел: нажатие отклонено");
                return SubmitOutcome::Rejected(SubmitRejection::Busy);
            }
            Err(AdmissionRefused::ServiceStopped) => {
                bump(&self.counters.rejected_service_stopped);
                return SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable);
            }
            Err(AdmissionRefused::DuplicateId(duplicate)) => {
                bump(&self.counters.rejected_duplicate_id);
                tracing::error!(target: LOG_TARGET, ?duplicate, "номер снимка уже в работе: ошибка нумерации");
                return SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable);
            }
        };

        // Запись ДО билета: фаза рабочего потока может прийти сразу после `begin_still`.
        self.ledger
            .register(id, request.request, self.ledger.current_screen_epoch());
        if self.service.begin_still(ticket).is_err() {
            // Билет уничтожен вместе с отказом (слот свободен), фазы не будет: запись убираем сами.
            self.ledger.unregister(id);
            bump(&self.counters.rejected_service_stopped);
            return SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable);
        }
        bump(&self.counters.accepted);
        self.record_in_journal(&request);
        tracing::info!(target: LOG_TARGET, ?id, request = ?request.request, pressed_at = ?request.pressed_at, "снимок принят");

        let begin = StillBegin {
            plan: plan_from_shot(id, &request),
            events: Arc::clone(&self.events) as Arc<dyn render_loop::still::StillEventSink>,
        };
        match self.render.send_still_command(StillCommand::Begin(begin)) {
            StillCommandDelivery::Delivered => {}
            refused => self.abort_still(id, refused),
        }
        SubmitOutcome::Accepted
    }
}

#[cfg(test)]
#[path = "still_capture_service_tests.rs"]
mod tests;
