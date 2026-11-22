//! Служба записи снимков на своём рабочем потоке `fasticamera-still` (решения 140–143).
//!
//! Архитектура:
//! * [`StillAdmission`] владеет учётом «сколько снимков ждут сохранения»; приложение берёт билет
//!   ДО отправки плана рендеру ([`StillServiceHandle::try_admit`]) и отдаёт его службе
//!   ([`StillServiceHandle::begin_still`]); слот освобождается при итоговой фазе снимка;
//! * рендер присылает события [`DevelopEvent`](still_contract::develop::DevelopEvent) через
//!   [`StillServiceHandle::submit_develop_event`] — это очередь с пределом, переполнение — явная
//!   ошибка, рендер никогда не ждёт. Фаза `Secured` уходит экрану в момент приёма события на
//!   потоке вызывающего (решение 140): рабочий поток занят файлами прошлых снимков и «кадр
//!   захвачен» задерживать не должен. Сроки снимков считаются от момента прихода команды в службу;
//! * рабочий поток открывает черновики и кодеры (не больше `max_open_encoders` одновременно, цель
//!   остальным выдаётся по очереди `Secured`) через сервисы оболочки, отвечает рендеру целью
//!   ([`PrimaryTargetSink`]), пишет DNG, публикует файлы и сообщает фазы ([`StillPhaseSink`]);
//! * запись форматов (DNG, Exif, вставка Exif в JPEG) спрятана за [`StillFileFormats`]: служба
//!   отвечает за порядок и учёт, а байты проверяют тесты самих форматов.
//!
//! Инварианты: частичный файл в галерее не появляется; слот допуска освобождается ровно один раз;
//! событие рендера для уже законченного снимка игнорируется и считается; рабочий поток вызывает
//! платформу, а поток рендера и главный поток Android — никогда.

mod admission;
mod config;
mod formats;
mod inbox;
mod media_writer;
mod naming;
mod ports;
mod stats;
mod worker;

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use shell_contract::still_media::{HeifEncoderService, JpegCompressor, MediaStoreService};
use still_contract::develop::DevelopEvent;
use still_contract::phase::StillPhase;
use still_contract::plan::StillId;

pub use admission::{AdmissionRefused, AdmissionTicket, SecuredAnnouncement, StillAdmission};
pub use config::{StillConfigError, StillServiceConfig};
pub use formats::{PhotoFileFormats, StillFileFormats};
pub use ports::{CameraCatalog, PrimaryTargetSink, RenderUnavailable, StillPhaseSink, WallClock};
pub use stats::StillServiceStats;

use inbox::{Inbox, InboxRejection};
use stats::StillCounters;
use worker::Worker;

/// Имя рабочего потока (видно в логах и профилировщике).
const WORKER_THREAD_NAME: &str = "fasticamera-still";

/// Всё, что служба получает снаружи.
pub struct StillServiceDeps {
    pub store: Arc<dyn MediaStoreService>,
    pub heif: Arc<dyn HeifEncoderService>,
    pub jpeg: Arc<dyn JpegCompressor>,
    pub formats: Arc<dyn StillFileFormats>,
    pub clock: Arc<dyn WallClock>,
    pub cameras: Arc<dyn CameraCatalog>,
    /// Приёмник фаз (экран).
    pub phases: Arc<dyn StillPhaseSink>,
    /// Канал к рендеру для цели основной картинки.
    pub targets: Arc<dyn PrimaryTargetSink>,
}

/// Служба не запустилась.
#[derive(Debug, thiserror::Error)]
pub enum StillServiceStartError {
    #[error(transparent)]
    InvalidConfig(#[from] StillConfigError),
    #[error("не удалось запустить поток службы снимков: {0}")]
    SpawnFailed(String),
}

/// Служба остановлена: принять команду некому.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("служба снимков остановлена")]
pub struct StillServiceStopped;

/// Событие рендера не принято. Отвергнутое событие уничтожено: его поверхность и картинка отпущены.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DevelopSubmitError {
    /// Очередь событий полна (счётчик `rejected_events`).
    #[error("очередь событий службы снимков полна")]
    QueueFull,
    #[error("служба снимков остановлена")]
    ServiceStopped,
}

/// Чем закончилась остановка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReport {
    /// Поток завершился, незаконченные снимки сорваны, черновики удалены.
    Stopped,
    /// Поток не завершился за срок (завис в платформенном вызове): он отпущен и закончит сам.
    WorkerStillRunning,
}

/// Дешёвая копируемая ручка к службе: её раздают приложению и рендеру.
#[derive(Clone)]
pub struct StillServiceHandle {
    inbox: Arc<Inbox>,
    admission: Arc<StillAdmission>,
    counters: Arc<StillCounters>,
    /// Приёмник фаз: сюда с потока вызывающего уходит только `Secured` (остальное — с рабочего потока).
    phases: Arc<dyn StillPhaseSink>,
}

impl StillServiceHandle {
    /// Допуск снимка (до отправки плана рендеру). Отказ ничего не трогает.
    pub fn try_admit(&self, id: StillId) -> Result<AdmissionTicket, AdmissionRefused> {
        self.admission.try_admit(id)
    }

    /// Отдаёт билет службе: она начинает ждать кадр этого снимка. Звать ДО отправки плана рендеру,
    /// иначе событие рендера может прийти раньше билета.
    pub fn begin_still(&self, ticket: AdmissionTicket) -> Result<(), StillServiceStopped> {
        self.inbox.push_ticket(ticket).map_err(|_| StillServiceStopped)
    }

    /// Событие проявления от рендера. Не блокирует.
    ///
    /// `Secured` сначала объявляется экрану прямо здесь (один раз на снимок, пока слот занят), и только
    /// потом событие встаёт в очередь рабочего потока. Объявление ДО постановки, а не после: иначе
    /// быстрый рабочий поток мог бы закончить снимок раньше, чем «кадр захвачен» дошёл бы до экрана.
    /// Если очередь потом откажет, снимок всё равно получит итог (срыв по сроку захвата), так что
    /// «Secured без итога» невозможен.
    pub fn submit_develop_event(&self, event: DevelopEvent) -> Result<(), DevelopSubmitError> {
        if let DevelopEvent::Secured { facts, .. } = &event {
            let id = facts.id;
            self.admission
                .announce_secured(id, || self.phases.report_phase(StillPhase::Secured { id }));
        }
        self.inbox.push_event(event).map_err(|rejection| match rejection {
            InboxRejection::Full => {
                self.counters.record_rejected_event();
                DevelopSubmitError::QueueFull
            }
            InboxRejection::Stopped => DevelopSubmitError::ServiceStopped,
        })
    }

    /// Снимок счётчиков.
    pub fn stats(&self) -> StillServiceStats {
        self.counters.snapshot(&self.admission)
    }
}

/// Владелец рабочего потока.
pub struct StillService {
    handle: StillServiceHandle,
    worker: Option<WorkerThread>,
    shutdown_timeout: std::time::Duration,
}

struct WorkerThread {
    join: JoinHandle<()>,
    /// Приходит, когда поток закончил (нужен ради ограниченного ожидания: `join` без срока нет).
    finished: Receiver<()>,
}

impl StillService {
    /// Проверяет конфиг и запускает рабочий поток.
    pub fn start(config: StillServiceConfig, deps: StillServiceDeps) -> Result<Self, StillServiceStartError> {
        config.validate()?;
        let admission = StillAdmission::new(config.max_pending_stills);
        let counters = Arc::new(StillCounters::default());
        let (command_sender, command_receiver) = mpsc::channel();
        let inbox = Arc::new(Inbox::new(command_sender, config.event_queue_capacity));
        let shutdown_timeout = config.shutdown_timeout();
        let phases = Arc::clone(&deps.phases);
        let worker = Worker::new(
            config,
            deps,
            Arc::clone(&counters),
            Arc::clone(&admission),
            Arc::clone(&inbox),
        );
        let (finished_sender, finished) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name(WORKER_THREAD_NAME.to_owned())
            .spawn(move || {
                worker.run(&command_receiver);
                // Получателя может не быть: `shutdown` перестал ждать. Сообщать некому — не ошибка.
                let _ = finished_sender.send(());
            })
            .map_err(|error| StillServiceStartError::SpawnFailed(error.to_string()))?;
        Ok(Self {
            handle: StillServiceHandle {
                inbox,
                admission,
                counters,
                phases,
            },
            worker: Some(WorkerThread { join, finished }),
            shutdown_timeout,
        })
    }

    pub fn handle(&self) -> StillServiceHandle {
        self.handle.clone()
    }

    pub fn stats(&self) -> StillServiceStats {
        self.handle.stats()
    }

    /// Останавливает службу: незаконченные снимки получают `Failed(ServiceUnavailable)`, черновики
    /// удаляются. Ждёт не дольше `shutdown_timeout_ms`.
    pub fn shutdown(mut self) -> ShutdownReport {
        self.stop()
    }

    fn stop(&mut self) -> ShutdownReport {
        let Some(worker) = self.worker.take() else {
            return ShutdownReport::Stopped;
        };
        // Сначала закрываем допуск: после этого новые билеты не выдаются.
        self.handle.admission.close();
        if !self.handle.inbox.push_shutdown() {
            // Поток уже завершился сам: остаётся собрать его.
            return join_finished(worker);
        }
        match worker.finished.recv_timeout(self.shutdown_timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => join_finished(worker),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                tracing::error!("поток службы снимков не остановился в срок, отпускаем его");
                ShutdownReport::WorkerStillRunning
            }
        }
    }
}

fn join_finished(worker: WorkerThread) -> ShutdownReport {
    if worker.join.join().is_err() {
        tracing::error!("поток службы снимков завершился паникой");
    }
    ShutdownReport::Stopped
}

impl Drop for StillService {
    fn drop(&mut self) {
        // Служба без явного `shutdown` не должна оставлять поток и черновики.
        self.stop();
    }
}
