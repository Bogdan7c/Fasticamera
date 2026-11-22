//! Подставные объекты для тестов маршрутизатора и сценариев: рендер, GPU-диагностики, маршрут событий.
//! Только для тестов.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use render_loop::mailbox::InboxDelivery;
use render_loop::pacing::{HISTOGRAM_BUCKETS, PacingSnapshot, PacingVerdict};
use render_loop::thread::{BackendHealth, RenderState, RenderStatus};
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceReleaseRequest, SurfaceSize};

use crate::gpu_services::{
    AhbCheckRequest, AhbCheckResult, FenceCheckRequest, FenceCheckResult, GpuDiagnostics, RenderObservation,
    RenderStatusSource, ValidationState,
};
use crate::render_route::RenderRoute;

/// Состояние рендера без неисправностей, показ идёт: точка отсчёта для правки в тестах.
pub fn presenting_status() -> RenderStatus {
    RenderStatus {
        state: RenderState::Presenting,
        generation: Some(SurfaceGeneration::new(1)),
        frames_presented: 1200,
        acquire_timeouts: 0,
        recreations: 0,
        surfaces_attached: 1,
        releases_confirmed: 0,
        last_release_latency: None,
        stale_changes: 0,
        backend_errors: 0,
        last_backend_error: None,
        pacing: PacingSnapshot {
            intervals: 1200,
            histogram: [0, 1200, 0, 0, 0, 0, 0, 0],
            misses: 0,
            miss_ratio: 0.0,
            observed_span: Duration::from_secs(10),
            timing_gaps: 0,
            frames_covered: 1201,
            rejected_out_of_order: 0,
            verdict: PacingVerdict::Passed,
        },
        pacing_refresh_period: Duration::from_secs_f64(1.0 / 120.0),
        display: Some(render_loop::backend::DisplayInfo {
            format: "B8G8R8A8_UNORM".to_owned(),
            color_space: "SRGB_NONLINEAR_KHR".to_owned(),
            extent: SurfaceSize {
                width_px: 1440,
                height_px: 3216,
            },
            transform: "Identity".to_owned(),
            image_count: 3,
            recreation_count: 0,
            refresh_cycle: Some(Duration::from_secs_f64(1.0 / 120.0)),
        }),
        backend: BackendHealth {
            live_objects: 40,
            live_objects_by_kind: Default::default(),
            validation_errors: 0,
            validation_warnings: 0,
            detach_failures: 0,
            known_deviations: Default::default(),
            camera: None,
            viewfinder: None,
        },
        frame_rate_request: Some(shell_contract::commands::FrameRateOutcome::Applied),
        screen_wake: render_loop::thread::ScreenWakeStatus {
            requested: shell_contract::commands::ScreenWake::KeepOn,
            last_outcome: Some(shell_contract::commands::CommandOutcome::Queued),
            commands_sent: 1,
        },
        fault: None,
        recent_presentations: Vec::new(),
        present_target: render_loop::thread::PresentTargetDiagnostics::default(),
        present_target_commands: render_loop::thread::PresentTargetCommandLog::default(),
        viewfinder_commands: render_loop::viewfinder::ViewfinderCommandLog::default(),
        still_commands: render_loop::still::StillCommandLog::default(),
        adpf: render_loop::thread::AdpfDiagnostics::default(),
        adpf_commands: render_loop::thread::AdpfCommandLog::default(),
    }
}

const _: () = assert!(
    HISTOGRAM_BUCKETS == 8,
    "presenting_status рассчитан на 8 корзин гистограммы"
);

/// Рендер, состояние которого тест меняет по ходу сценария.
pub struct FakeRenderStatus {
    observation: Mutex<RenderObservation>,
}

impl FakeRenderStatus {
    pub fn live(status: RenderStatus, validation: ValidationState) -> Arc<Self> {
        Arc::new(Self {
            observation: Mutex::new(RenderObservation::Live {
                status: Box::new(status),
                validation,
            }),
        })
    }

    pub fn unavailable(reason: &str) -> Arc<Self> {
        Arc::new(Self {
            observation: Mutex::new(RenderObservation::Unavailable {
                reason: reason.to_owned(),
            }),
        })
    }

    /// Меняет статус (рендер должен быть «живым»).
    pub fn update(&self, change: impl FnOnce(&mut RenderStatus)) {
        let mut observation = self.observation.lock().expect("замок");
        if let RenderObservation::Live { status, .. } = &mut *observation {
            change(status);
        }
    }
}

impl RenderStatusSource for FakeRenderStatus {
    fn observe(&self) -> RenderObservation {
        self.observation.lock().expect("замок").clone()
    }
}

/// Что получил маршрут.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutedEvent {
    SurfaceCreated(u64),
    SurfaceChanged(u64, SurfaceSize),
    SurfaceDestroyed(u64),
    Paused,
    Resumed,
}

/// Как маршрут обращается с квитанцией освобождения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptPolicy {
    /// Подтверждает сразу (как поток рендера после detach).
    Confirm,
    /// Держит у себя (поток рендера ещё не отпустил surface): оболочка ждёт.
    Hold,
}

/// Маршрут-запись: хранит surface у себя (владелец — получатель, не маршрутизатор).
pub struct RecordingRoute {
    pub log: Arc<Mutex<Vec<RoutedEvent>>>,
    pub held_surfaces: Arc<Mutex<Vec<NativeSurface>>>,
    pub held_receipts: Arc<Mutex<Vec<SurfaceReleaseRequest>>>,
    pub receipts: ReceiptPolicy,
    pub delivery: InboxDelivery,
}

impl RecordingRoute {
    pub fn new(receipts: ReceiptPolicy) -> Self {
        Self {
            log: Arc::default(),
            held_surfaces: Arc::default(),
            held_receipts: Arc::default(),
            receipts,
            delivery: InboxDelivery::Delivered,
        }
    }

    fn record(&self, event: RoutedEvent) -> InboxDelivery {
        self.log.lock().expect("замок").push(event);
        self.delivery
    }
}

impl RenderRoute for RecordingRoute {
    fn surface_created(&mut self, surface: NativeSurface) -> InboxDelivery {
        let generation = surface.generation().value();
        self.held_surfaces.lock().expect("замок").push(surface);
        self.record(RoutedEvent::SurfaceCreated(generation))
    }

    fn surface_changed(&mut self, generation: SurfaceGeneration, size: SurfaceSize) -> InboxDelivery {
        self.record(RoutedEvent::SurfaceChanged(generation.value(), size))
    }

    fn surface_destroyed(&mut self, request: SurfaceReleaseRequest) -> InboxDelivery {
        let generation = request.generation();
        self.held_surfaces
            .lock()
            .expect("замок")
            .retain(|surface| surface.generation() != generation);
        match self.receipts {
            ReceiptPolicy::Confirm => request.confirm_released(),
            ReceiptPolicy::Hold => self.held_receipts.lock().expect("замок").push(request),
        }
        self.record(RoutedEvent::SurfaceDestroyed(generation.value()))
    }

    fn paused(&mut self) -> InboxDelivery {
        self.record(RoutedEvent::Paused)
    }

    fn resumed(&mut self) -> InboxDelivery {
        self.record(RoutedEvent::Resumed)
    }
}

/// GPU-диагностики с заранее заданными ответами; запоминают запросы.
pub struct FakeGpuDiagnostics {
    ahb_answers: Mutex<VecDeque<AhbCheckResult>>,
    fence_answer: Mutex<Option<FenceCheckResult>>,
    pub ahb_requests: Mutex<Vec<AhbCheckRequest>>,
    /// Сколько раз вызвана пачка проверок AHB (одна пачка = один GPU-контекст).
    pub ahb_sessions: AtomicUsize,
    pub fence_requests: Mutex<Vec<FenceCheckRequest>>,
    /// Если задан, КАЖДЫЙ вызов диагностики блокируется, пока тест не пошлёт сигнал или не
    /// закроет отправителя (имитация зависшего драйвера/fence).
    hang_until_released: Option<Mutex<Receiver<()>>>,
}

impl FakeGpuDiagnostics {
    /// `ahb_answers` отдаются по очереди (RAW16, потом RAW10).
    pub fn new(ahb_answers: Vec<AhbCheckResult>, fence_answer: Option<FenceCheckResult>) -> Arc<Self> {
        Arc::new(Self {
            ahb_answers: Mutex::new(ahb_answers.into()),
            fence_answer: Mutex::new(fence_answer),
            ahb_requests: Mutex::default(),
            ahb_sessions: AtomicUsize::new(0),
            fence_requests: Mutex::default(),
            hang_until_released: None,
        })
    }

    /// Диагностики, которые зависают в первом же вызове. Отправитель держит тест: пока он
    /// жив, вызов не вернётся; при `drop` рабочий поток сценария разблокируется и завершится.
    pub fn hanging() -> (Arc<Self>, Sender<()>) {
        let (release, gate) = channel();
        let diagnostics = Arc::new(Self {
            ahb_answers: Mutex::default(),
            fence_answer: Mutex::default(),
            ahb_requests: Mutex::default(),
            ahb_sessions: AtomicUsize::new(0),
            fence_requests: Mutex::default(),
            hang_until_released: Some(Mutex::new(gate)),
        });
        (diagnostics, release)
    }

    fn hang_if_requested(&self) {
        if let Some(gate) = &self.hang_until_released {
            // Ошибка recv (отправитель закрыт) тоже отпускает: тест закончился.
            let _ = gate.lock().expect("замок").recv();
        }
    }
}

impl GpuDiagnostics for FakeGpuDiagnostics {
    fn ahb_roundtrips(
        &self,
        requests: &[AhbCheckRequest],
        on_checked: &mut dyn FnMut(&AhbCheckRequest),
    ) -> Vec<AhbCheckResult> {
        self.ahb_sessions.fetch_add(1, Ordering::SeqCst);
        requests
            .iter()
            .map(|request| {
                self.ahb_requests.lock().expect("замок").push(*request);
                self.hang_if_requested();
                let answer = self
                    .ahb_answers
                    .lock()
                    .expect("замок")
                    .pop_front()
                    .unwrap_or(AhbCheckResult::Failed {
                        reason: "ответ не задан".to_owned(),
                    });
                on_checked(request);
                answer
            })
            .collect()
    }

    fn fence_order(&self, request: FenceCheckRequest) -> FenceCheckResult {
        self.fence_requests.lock().expect("замок").push(request);
        self.hang_if_requested();
        self.fence_answer
            .lock()
            .expect("замок")
            .take()
            .unwrap_or(FenceCheckResult::Failed {
                reason: "ответ не задан".to_owned(),
            })
    }
}

/// Регулировки рендера, которые записывают каждый запрос и отвечают «доставлено».
#[derive(Default)]
pub struct RecordingRenderControl {
    pub adjustments: Mutex<Vec<crate::viewfinder_control::RenderAdjustment>>,
    /// Команды UI-проходу по порядку (источник — только по имени: он не `Clone`).
    pub ui_overlay_commands: Mutex<Vec<RecordedUiOverlay>>,
    /// Установленный сейчас источник UI (как слот потока рендера): `Install` кладёт, `Remove` убирает.
    /// Тест сам «рисует кадры» этим источником через [`RecordingRenderControl::produce_ui_frame`].
    installed_ui_source: Mutex<Option<Box<dyn ui_draw::frame_source::UiFrameSource>>>,
}

/// Команда UI-проходу, как её записал [`RecordingRenderControl`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedUiOverlay {
    /// Поставлен источник с этим именем.
    Install(&'static str),
    /// Источник снят.
    Remove,
}

impl RecordingRenderControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Все запросы по порядку.
    pub fn recorded(&self) -> Vec<crate::viewfinder_control::RenderAdjustment> {
        self.adjustments.lock().expect("замок").clone()
    }

    /// Один «кадр рендера» установленным источником UI; `None` — источника сейчас нет.
    pub fn produce_ui_frame(
        &self,
        request: &ui_draw::frame_source::UiFrameRequest,
    ) -> Option<Result<ui_draw::draw_list::DrawList, ui_draw::frame_source::UiFrameError>> {
        let mut slot = self.installed_ui_source.lock().expect("замок");
        slot.as_mut().map(|source| source.produce_frame(request))
    }

    /// Имя установленного сейчас источника UI.
    pub fn installed_ui_source_name(&self) -> Option<&'static str> {
        self.installed_ui_source
            .lock()
            .expect("замок")
            .as_ref()
            .map(|source| source.name())
    }

    /// Все команды UI-проходу по порядку.
    pub fn recorded_ui_overlay(&self) -> Vec<RecordedUiOverlay> {
        self.ui_overlay_commands.lock().expect("замок").clone()
    }
}

impl crate::viewfinder_control::RenderControl for RecordingRenderControl {
    fn adjust(
        &self,
        adjustment: crate::viewfinder_control::RenderAdjustment,
    ) -> crate::viewfinder_control::ControlDelivery {
        self.adjustments.lock().expect("замок").push(adjustment);
        crate::viewfinder_control::ControlDelivery::Delivered
    }

    fn ui_overlay(
        &self,
        command: render_loop::ui_overlay::UiOverlayCommand,
    ) -> crate::viewfinder_control::ControlDelivery {
        let recorded = match command {
            render_loop::ui_overlay::UiOverlayCommand::Install(source) => {
                let name = source.name();
                *self.installed_ui_source.lock().expect("замок") = Some(source);
                RecordedUiOverlay::Install(name)
            }
            render_loop::ui_overlay::UiOverlayCommand::Remove => {
                *self.installed_ui_source.lock().expect("замок") = None;
                RecordedUiOverlay::Remove
            }
        };
        self.ui_overlay_commands.lock().expect("замок").push(recorded);
        crate::viewfinder_control::ControlDelivery::Delivered
    }
}
