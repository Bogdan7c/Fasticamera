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
//!    освободился бы лишь по сроку захвата;
//! 5. только для снимка со вспышкой (14d) и ТОЛЬКО после `Begin`: команда камере `capture_flash_still`. Рендер
//!    игнорирует срыв вспышки, пришедший раньше `Begin`, поэтому порядок «рендер, потом камера» нарушать нельзя.
//!    Команда камере не дошла (очередь полна, движок остановлен) — рендеру уходит `FlashFailed(CommandNotDelivered)`,
//!    и рендер даёт единственный итог `NotSecured`.
//!
//! # Вспышка (14d)
//! Маршрут снимка (со вспышкой или показанный кадр) решает [`FlashRoute::decide`] по режиму экрана и совету камеры
//! для Auto. Выбор и причина пишутся в счётчики и журнал маршрутов ([`StillCaptureService::flash_routes`]).
//! Дальнейшая судьба кадра со вспышкой (камера → кольцо → рендер) — не здесь: итог камеры возвращает
//! [`crate::flash_still_relay::FlashStillRelay`].
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

use camera_contract::flash::FlashStillFailure;
use camera_screen::settings::FlashMode;
use camera_screen::shutter::ShotRequest;
use photo_output::service::{AdmissionRefused, StillServiceHandle};
use render_loop::still::{StillBegin, StillCommand};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::plan::{FrameSelection, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};

use crate::camera_screen_capture::{CaptureService, SubmitOutcome, SubmitRejection};
use crate::flash_camera_port::{FlashCameraPort, flash_shot_of};
use crate::flash_shot_route::{FlashReason, FlashRoute, NoFlashReason};
use crate::still_event_forwarder::DevelopEventForwarder;
use crate::still_render_port::{StillCommandDelivery, StillRenderPort};
use crate::still_screen_ledger::StillScreenLedger;

const LOG_TARGET: &str = "fasticamera::still::capture";

/// План снимка из запроса экрана. Всё, что меняется во времени (зум, соотношение, поворот), зафиксировано
/// запросом на момент нажатия, поэтому план его только переводит на язык рендера.
///
/// Какой кадр брать (показанный при нажатии или кадр со вспышкой) решает вызывающий ([`FlashRoute`]): план только
/// переводит решение на язык рендера.
pub fn plan_from_shot(id: StillId, request: &ShotRequest, selection: FrameSelection) -> StillPlan {
    StillPlan {
        id,
        press: ShutterPress { at: request.pressed_at },
        selection,
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
    /// Снимки со вспышкой по режиму `On`.
    pub flash_mode_on: u64,
    /// `Auto`: камера посоветовала вспышку, снимок пошёл со вспышкой.
    pub flash_auto_fired: u64,
    /// `Auto`: света хватает, снимок пошёл без вспышки.
    pub flash_auto_not_needed: u64,
    /// `Auto`: совета камеры нет, снимок пошёл без вспышки.
    pub flash_auto_advice_unavailable: u64,
    /// Команда камере «снять со вспышкой» не дошла до движка: снимок сорван через рендер.
    pub flash_command_rejected: u64,
    /// Срыв вспышки не удалось сообщить рендеру (он завершён): итог даст срок службы.
    pub flash_failure_report_lost: u64,
}

/// Запись журнала маршрутов: какой режим экрана и какой маршрут получил принятый снимок.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlashRouteEntry {
    pub still: StillId,
    pub mode: FlashModeName,
    pub uses_flash: bool,
    /// Причина маршрута словами (для отчёта сценария).
    pub why: &'static str,
}

/// Имя режима вспышки для журнала (сам `FlashMode` экрана не сериализуется).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashModeName {
    Off,
    Auto,
    On,
}

impl From<FlashMode> for FlashModeName {
    fn from(mode: FlashMode) -> Self {
        match mode {
            FlashMode::Off => Self::Off,
            FlashMode::Auto => Self::Auto,
            FlashMode::On => Self::On,
        }
    }
}

#[derive(Default)]
struct CounterCells {
    accepted: AtomicU64,
    rejected_queue_full: AtomicU64,
    rejected_service_stopped: AtomicU64,
    rejected_duplicate_id: AtomicU64,
    render_refused: AtomicU64,
    abort_event_lost: AtomicU64,
    flash_mode_on: AtomicU64,
    flash_auto_fired: AtomicU64,
    flash_auto_not_needed: AtomicU64,
    flash_auto_advice_unavailable: AtomicU64,
    flash_command_rejected: AtomicU64,
    flash_failure_report_lost: AtomicU64,
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
    /// Камера для вспышки: совет для Auto и команда снимка со вспышкой.
    pub flash_camera: Arc<dyn FlashCameraPort>,
    /// Ёмкость журнала принятых запросов (не меньше 1).
    pub journal_capacity: usize,
}

/// Реализация [`CaptureService`] поверх службы записи и потока рендера.
pub struct StillCaptureService {
    service: StillServiceHandle,
    render: Arc<dyn StillRenderPort>,
    ledger: Arc<StillScreenLedger>,
    events: Arc<DevelopEventForwarder>,
    flash_camera: Arc<dyn FlashCameraPort>,
    /// Последний выданный номер: номера сквозные на процесс и начинаются с 1.
    last_id: AtomicU64,
    journal: Mutex<VecDeque<ShotRequest>>,
    /// Маршруты принятых снимков (той же ёмкости, что журнал запросов).
    flash_routes: Mutex<VecDeque<FlashRouteEntry>>,
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
            flash_camera: parts.flash_camera,
            last_id: AtomicU64::new(0),
            journal: Mutex::default(),
            flash_routes: Mutex::default(),
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
            flash_mode_on: load(&self.counters.flash_mode_on),
            flash_auto_fired: load(&self.counters.flash_auto_fired),
            flash_auto_not_needed: load(&self.counters.flash_auto_not_needed),
            flash_auto_advice_unavailable: load(&self.counters.flash_auto_advice_unavailable),
            flash_command_rejected: load(&self.counters.flash_command_rejected),
            flash_failure_report_lost: load(&self.counters.flash_failure_report_lost),
        }
    }

    /// Маршруты последних принятых снимков по порядку (старые вытеснены).
    pub fn flash_routes(&self) -> Vec<FlashRouteEntry> {
        self.flash_routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }

    /// Решает маршрут снимка, считает его причину и пишет в журнал маршрутов.
    fn route_shot(&self, id: StillId, request: &ShotRequest) -> FlashRoute {
        // Совет камеры читается только для Auto (см. `FlashRoute::decide`).
        let route = FlashRoute::decide(request.flash, || self.flash_camera.flash_status().advice);
        let (counter, why) = match route {
            FlashRoute::FlashFrame(FlashReason::ModeOn) => (Some(&self.counters.flash_mode_on), "режим On"),
            FlashRoute::FlashFrame(FlashReason::AutoAdvisedFire) => {
                (Some(&self.counters.flash_auto_fired), "Auto: камера советует вспышку")
            }
            FlashRoute::ShownAtPress(NoFlashReason::ModeOff) => (None, "режим Off"),
            FlashRoute::ShownAtPress(NoFlashReason::AutoNotNeeded) => {
                (Some(&self.counters.flash_auto_not_needed), "Auto: света хватает")
            }
            FlashRoute::ShownAtPress(NoFlashReason::AutoAdviceUnavailable(_)) => (
                Some(&self.counters.flash_auto_advice_unavailable),
                "Auto: совета камеры нет",
            ),
        };
        if let Some(cell) = counter {
            bump(cell);
        }
        tracing::info!(target: LOG_TARGET, ?id, mode = ?request.flash, ?route, why, "маршрут снимка выбран");
        let mut routes = self.flash_routes.lock().unwrap_or_else(PoisonError::into_inner);
        if routes.len() >= self.journal_capacity {
            routes.pop_front();
        }
        routes.push_back(FlashRouteEntry {
            still: id,
            mode: request.flash.into(),
            uses_flash: route.uses_flash(),
            why,
        });
        route
    }

    /// Просит камеру снять кадр со вспышкой. Отказ доставки — не тихий: рендер получает срыв и завершает снимок.
    fn start_flash_still(&self, id: StillId) {
        let shot = flash_shot_of(id);
        let Err(rejected) = self.flash_camera.capture_flash_still(shot) else {
            return;
        };
        bump(&self.counters.flash_command_rejected);
        tracing::warn!(target: LOG_TARGET, ?id, %rejected, "команда вспышки не дошла до камеры: срываем снимок");
        let failure = FlashStillFailure::CommandNotDelivered;
        match self
            .render
            .send_still_command(StillCommand::FlashFailed { id, failure })
        {
            StillCommandDelivery::Delivered => {}
            lost => {
                bump(&self.counters.flash_failure_report_lost);
                tracing::warn!(target: LOG_TARGET, ?id, ?lost, "рендер не принял срыв вспышки: итог даст срок службы");
            }
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

        let route = self.route_shot(id, &request);
        let begin = StillBegin {
            plan: plan_from_shot(id, &request, route.selection(flash_shot_of(id))),
            events: Arc::clone(&self.events) as Arc<dyn render_loop::still::StillEventSink>,
        };
        match self.render.send_still_command(StillCommand::Begin(begin)) {
            // Команда камере строго ПОСЛЕ `Begin`: срыв вспышки до `Begin` рендер проигнорировал бы.
            StillCommandDelivery::Delivered if route.uses_flash() => self.start_flash_still(id),
            StillCommandDelivery::Delivered => {}
            refused => self.abort_still(id, refused),
        }
        SubmitOutcome::Accepted
    }
}

#[cfg(test)]
#[path = "still_capture_service_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "still_capture_flash_tests.rs"]
mod flash_tests;
