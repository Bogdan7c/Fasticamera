//! Сборка настоящего снимка: служба записи, сервис снимка, таблица «снимок → экран» и шов к рендеру.
//!
//! Модуль нейтрален к платформе: платформенные сервисы (MediaStore, кодеры, звук) и шов к рендеру приходят
//! готовыми трейтами, поэтому весь путь «нажатие → план рендеру → события → файл → экран» проверяется на ПК
//! настоящей службой `photo-output` с подделками оболочки и рендера. Android-обвязка (`shell_wiring`) только
//! подставляет настоящие сервисы.
//!
//! ```text
//! экран ─ShotRequest─▶ StillCaptureService ─Begin(plan, events)─▶ поток рендера
//!                          │ try_admit/begin_still                      │ DevelopEvent
//!                          ▼                                            ▼
//!                   StillService (рабочий поток) ◀─DevelopEventForwarder─┘
//!                          │ StillPhase                  ▲
//!                          ▼                             └─ Target(offer) ◀─ RenderTargetRoute
//!                   StillScreenLedger ─CaptureSecured / CaptureFinished─▶ канал входа ТЕКУЩЕГО экрана
//! ```
//!
//! Владение: [`StillService`] держит рабочий поток, его `Drop` останавливает поток (до `shutdown_timeout_ms`),
//! поэтому владелец — процессная обвязка, а не экран и не поток рендера.

use std::sync::Arc;

use android_shell::still_orphan_sweep::OrphanSweepReport;
use photo_output::service::{
    CameraCatalog, StillFileFormats, StillService, StillServiceDeps, StillServiceHandle, StillServiceStartError,
    StillServiceStats, WallClock,
};
use shell_contract::still_media::{HeifEncoderService, JpegCompressor, MediaStoreService, ShutterSoundService};

use crate::camera_screen_capture::{CaptureCompletionSlot, CaptureService};
use crate::camera_screen_config::CameraScreenConfig;
use crate::capture_backend::{
    CaptureBackend, CaptureDiagnostics, CaptureMode, CaptureMonitor, CaptureWiring, RealBackendParts, RealDiagnostics,
};
use crate::photo_config::PhotoConfig;
use crate::still_capture_service::{StillCaptureParts, StillCaptureService};
use crate::still_choice_log::ChoiceEntry;
use crate::still_event_forwarder::DevelopEventForwarder;
use crate::still_phase_log::{PhaseBatch, PhaseCursor};
use crate::still_render_port::{RenderTargetRoute, StillRenderPort};
use crate::still_screen_ledger::StillScreenLedger;

/// Откуда диагностика берёт итог уборки осиротевших черновиков (на телефоне — MediaStore-сервис оболочки).
pub trait OrphanSweepSource: Send + Sync {
    fn orphan_sweep_report(&self) -> OrphanSweepReport;
}

/// Всё, что настоящий снимок получает снаружи.
pub struct StillRuntimeDeps {
    pub store: Arc<dyn MediaStoreService>,
    pub heif: Arc<dyn HeifEncoderService>,
    pub jpeg: Arc<dyn JpegCompressor>,
    pub formats: Arc<dyn StillFileFormats>,
    pub clock: Arc<dyn WallClock>,
    pub cameras: Arc<dyn CameraCatalog>,
    /// Шов к потоку рендера (команды снимка и цель основной картинки).
    pub render: Arc<dyn StillRenderPort>,
    pub shutter_sound: Arc<dyn ShutterSoundService>,
    pub orphan_sweep: Arc<dyn OrphanSweepSource>,
}

/// Собранный снимок: владелец службы и набор для экрана.
pub struct StillRuntime {
    service: StillService,
    backend: CaptureBackend,
}

impl StillRuntime {
    /// Проверяет конфиг службы и запускает её рабочий поток.
    pub fn start(config: &PhotoConfig, deps: StillRuntimeDeps) -> Result<Self, StillServiceStartError> {
        let completion_slot = Arc::new(CaptureCompletionSlot::default());
        let ledger = Arc::new(StillScreenLedger::new(
            Arc::clone(&completion_slot),
            config.ledger_capacity(),
        ));
        let service = StillService::start(
            config.service.clone(),
            StillServiceDeps {
                store: deps.store,
                heif: deps.heif,
                jpeg: deps.jpeg,
                formats: deps.formats,
                clock: deps.clock,
                cameras: deps.cameras,
                phases: Arc::clone(&ledger) as Arc<dyn photo_output::service::StillPhaseSink>,
                targets: Arc::new(RenderTargetRoute::new(Arc::clone(&deps.render))),
            },
        )?;
        let handle = service.handle();
        let events = Arc::new(DevelopEventForwarder::new(handle.clone()));
        let capture = Arc::new(StillCaptureService::new(StillCaptureParts {
            service: handle.clone(),
            render: deps.render,
            ledger: Arc::clone(&ledger),
            events: Arc::clone(&events),
            journal_capacity: config.journal_capacity,
        }));
        let monitor = Arc::new(RealMonitor {
            handle,
            capture: Arc::clone(&capture),
            events,
            ledger,
            orphan_sweep: deps.orphan_sweep,
        });
        let backend = CaptureBackend::real(RealBackendParts {
            service: capture as Arc<dyn CaptureService>,
            completion_slot,
            shutter_sound: deps.shutter_sound,
            monitor,
        });
        Ok(Self { service, backend })
    }

    /// Счётчики службы записи.
    pub fn stats(&self) -> StillServiceStats {
        self.service.stats()
    }

    /// Раздаёт части: службу (её держит процессная обвязка) и набор для экрана.
    pub fn into_parts(self) -> (StillService, CaptureBackend) {
        (self.service, self.backend)
    }
}

/// Что получает процессная обвязка: службу (её `Drop` останавливает рабочий поток, поэтому её держит процесс)
/// и набор для экрана.
pub struct StillAssembly {
    /// Служба записи; `None` — работает фейк (причина в диагностике набора).
    pub service: Option<StillService>,
    pub capture: CaptureWiring,
}

/// Поднимает настоящий снимок или, если не получилось, ЯВНЫЙ запасной фейк.
///
/// `prerequisites` — результат сбора зависимостей composition root: `Err(причина)` значит «нет рендера, камеры
/// или паспорта телефона». Запуск службы с неверным конфигом или без потока — тоже запасной фейк. Во всех случаях
/// причина попадает в лог и диагностику набора ([`CaptureMode::FakeFallback`]), а не теряется.
pub fn start_still_or_fallback(
    photo: &PhotoConfig,
    screen: &CameraScreenConfig,
    prerequisites: Result<StillRuntimeDeps, String>,
) -> StillAssembly {
    let deps = match prerequisites {
        Ok(deps) => deps,
        Err(reason) => return fallback(screen, reason),
    };
    match StillRuntime::start(photo, deps) {
        Ok(runtime) => {
            let (service, backend) = runtime.into_parts();
            StillAssembly {
                service: Some(service),
                capture: CaptureWiring::Backend(backend),
            }
        }
        Err(error) => fallback(screen, format!("служба снимков не запустилась: {error}")),
    }
}

fn fallback(screen: &CameraScreenConfig, reason: String) -> StillAssembly {
    StillAssembly {
        service: None,
        capture: CaptureWiring::Backend(CaptureBackend::fake_fallback(screen, reason)),
    }
}

/// Монитор настоящего режима.
struct RealMonitor {
    handle: StillServiceHandle,
    capture: Arc<StillCaptureService>,
    events: Arc<DevelopEventForwarder>,
    ledger: Arc<StillScreenLedger>,
    orphan_sweep: Arc<dyn OrphanSweepSource>,
}

impl CaptureMonitor for RealMonitor {
    fn diagnostics(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            mode: CaptureMode::Real,
            fake: None,
            real: Some(RealDiagnostics {
                service: self.handle.stats(),
                capture: self.capture.counters(),
                events: self.events.counters(),
                screen: self.ledger.counters(),
                orphan_drafts: self.orphan_sweep.orphan_sweep_report(),
            }),
        }
    }

    fn journal(&self) -> Vec<camera_screen::shutter::ShotRequest> {
        self.capture.journal()
    }

    fn accepted_total(&self) -> u64 {
        self.capture.counters().accepted
    }

    fn phases_after(&self, cursor: PhaseCursor) -> PhaseBatch {
        self.ledger.phase_log().entries_after(cursor)
    }

    fn phase_cursor_now(&self) -> PhaseCursor {
        self.ledger.phase_log().cursor_now()
    }

    fn frame_choices(&self) -> Vec<ChoiceEntry> {
        self.events.choice_log().snapshot()
    }
}
