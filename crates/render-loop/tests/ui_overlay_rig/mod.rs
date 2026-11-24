//! Стенд тестов UI поверх видоискателя (`vulkan_ui_overlay.rs`): бэкенд камеры с ручным управлением кадрами
//! показа, детерминированные кадры камеры и тестовый источник кадра UI со стеклом.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use super::camera_rig::{self, EventLog, FakeFrame, ProducerBook, TestImporter, result_for, stream_passport};
use super::support::{Journal, fake_surface};
use super::vulkan_support::{
    assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_keeping_unchanged_policy_or_skip,
    headless_camera_backend_with_white_or_skip, held_white,
};
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::sync::GpuWaitFence;
use raw_frame_stream::{OfferedFrame, RawFrameStream, RawFrameStreamConfig, StreamEpoch};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentOutcome, PresentTarget};
use render_loop::config::{RenderLoopConfig, UiOverlayConfig};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayDiagnostics, UiOverlayOutcome};
use render_loop::viewfinder::ViewfinderDiagnostics;
use render_loop::vulkan_backend::{ViewfinderReadback, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, GlassGroupItem, GlassMember, ImageClip, ImageFit, ImageItem, PressState};
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::glass::GlassStyle;
use ui_draw::light::LightDirection;
use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

/// Портретный экран: UI рассчитан на портрет, а `px_per_pt = ширина / 440`.
pub const SURFACE: SurfaceSize = SurfaceSize {
    width_px: 264,
    height_px: 480,
};
/// Другой размер для проверки пересоздания цепочки под работающим UI.
pub const RESIZED_SURFACE: SurfaceSize = SurfaceSize {
    width_px: 220,
    height_px: 400,
};
/// Сколько кадров показа ждать, пока новый кадр камеры дойдёт до ISP.
pub const MAX_FRAMES_TO_SETTLE: u64 = 20;
/// Шаг ожидаемого времени показа между кадрами показа, нс (метка проверяется в запросе источника).
pub const PRESENT_STEP_NANOS: i64 = 1_000_000;
pub const SOURCE_NAME: &str = "glass-test";

// ---- тестовый источник кадра UI ----

/// Что видел источник и как он должен отвечать.
#[derive(Default)]
pub struct SourceProbe {
    pub calls: AtomicU64,
    /// Источник отказывает, пока флаг поднят.
    pub failing: AtomicBool,
    /// Сколько групп стекла строить в кадре (по умолчанию одна).
    pub groups: AtomicU64,
    /// Все запросы источника по порядку.
    pub requests: Mutex<Vec<UiFrameRequest>>,
    /// Источник уничтожен (для проверки «уничтожается на потоке рендера, а не утекает»).
    pub dropped: AtomicBool,
}

impl SourceProbe {
    pub fn new() -> Arc<Self> {
        let probe = Self::default();
        probe.groups.store(1, Ordering::SeqCst);
        Arc::new(probe)
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<UiFrameRequest> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

pub struct GlassSource {
    probe: Arc<SourceProbe>,
}

impl Drop for GlassSource {
    fn drop(&mut self) {
        self.probe.dropped.store(true, Ordering::SeqCst);
    }
}

impl UiFrameSource for GlassSource {
    fn name(&self) -> &'static str {
        SOURCE_NAME
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.probe.calls.fetch_add(1, Ordering::SeqCst);
        self.probe
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(*request);
        if self.probe.failing.load(Ordering::SeqCst) {
            return Err(UiFrameError::SourceFailed {
                reason: "тестовый отказ".to_owned(),
            });
        }
        Ok(glass_list(request, self.probe.groups.load(Ordering::SeqCst)))
    }
}

/// Список из `groups` капсул стекла друг под другом.
pub fn glass_list(request: &UiFrameRequest, groups: u64) -> DrawList {
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    let size = PxSize::new(request.logical_size.width * 0.6, request.logical_size.height * 0.1);
    for index in 0..groups {
        let center_y = request.logical_size.height * (0.3 + 0.15 * index as f32);
        list.items.push(DrawItem::Glass(GlassGroupItem {
            params: GlassStyle::default().resolve(size, request.px_per_pt),
            members: vec![GlassMember {
                geometry: ShapeGeometry::Capsule { size },
                transform: Affine2::translate(request.logical_size.width * 0.5, center_y),
                press: PressState::RELEASED,
            }],
            merge_radius_px: 0.0,
            effect: 1.0,
            opacity: Opacity::OPAQUE,
        }));
    }
    list
}

// ---- источник кадра UI с картинкой (14g) ----

/// Что отдаёт источник с картинкой: картинку можно подменить на лету (или убрать — тогда список пуст).
#[derive(Default)]
pub struct PictureProbe {
    pub picture: Mutex<Option<UiPicture>>,
    pub calls: AtomicU64,
}

impl PictureProbe {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn show(&self, picture: Option<UiPicture>) {
        *self.picture.lock().unwrap_or_else(PoisonError::into_inner) = picture;
    }
}

/// Источник: круглая миниатюра по центру экрана (если картинка задана), больше ничего.
pub struct PictureSource {
    probe: Arc<PictureProbe>,
}

impl UiFrameSource for PictureSource {
    fn name(&self) -> &'static str {
        "picture-test"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.probe.calls.fetch_add(1, Ordering::SeqCst);
        let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
        let picture = self
            .probe
            .picture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(picture) = picture {
            let side = request.logical_size.width * 0.4;
            list.items.push(DrawItem::Image(ImageItem {
                source: picture.into(),
                transform: Affine2::translate(request.logical_size.width * 0.5, request.logical_size.height * 0.5),
                size_px: PxSize::new(side, side * 0.75),
                fit: ImageFit::Cover,
                clip: ImageClip::Circle,
                opacity: Opacity::OPAQUE,
            }));
        }
        Ok(list)
    }
}

/// Картинка-градиент `width × height` (разные пиксели, чтобы это была не «пустая» текстура).
pub fn test_picture(width: u32, height: u32) -> UiPicture {
    let mut bytes = Vec::new();
    for y in 0..height {
        for x in 0..width {
            bytes.extend_from_slice(&[(x * 255 / (width - 1)) as u8, (y * 255 / (height - 1)) as u8, 128, 255]);
        }
    }
    UiPicture::from_rgba8(PictureSize::new(width, height), PictureEncoding::Srgb, bytes).expect("картинка")
}

pub fn install_picture_command(probe: &Arc<PictureProbe>) -> UiOverlayCommand {
    UiOverlayCommand::Install(Box::new(PictureSource {
        probe: Arc::clone(probe),
    }))
}

pub fn install_command(probe: &Arc<SourceProbe>) -> UiOverlayCommand {
    UiOverlayCommand::Install(Box::new(GlassSource {
        probe: Arc::clone(probe),
    }))
}

// ---- стенд: бэкенд, поток кадров, ручное управление кадрами показа ----

pub fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(PRESENT_STEP_NANOS * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

/// Как стенд относится к пропуску тиков без изменений (решение 214).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnchangedPolicyInRig {
    /// Каждый тик рисуется и показывается: стенды, которым важен каждый показ (`present` ждёт `Presented`).
    EveryTickDrawn,
    /// Политика ровно из конфига потока рендера (по умолчанию пропуск включён): тесты самого пропуска.
    FromConfig,
}

pub struct Harness {
    pub backend: VulkanDisplayBackend,
    pub stream: RawFrameStream,
    pub epoch: StreamEpoch,
    pub log: Arc<EventLog>,
    pub book: Arc<ProducerBook>,
    pub journal: Arc<Journal>,
    pub baseline: gpu_vulkan::live_objects::LiveObjectSnapshot,
    pub frame_index: u64,
    pub sequence: i64,
}

impl Harness {
    /// Бэкенд камеры с UI-конфигом `ui_config`, поток кадров начат, surface ещё не присоединена.
    pub fn start(ui_config: UiOverlayConfig) -> Option<Self> {
        Self::start_with_initial_look(ui_config, None)
    }

    /// Бэкенд камеры, у которого пропуск тиков без изменений работает по конфигу `render_config` (по умолчанию
    /// включён): тик без нового кадра камеры и без изменений UI возвращает `SkippedUnchanged`.
    pub fn start_skipping_unchanged(render_config: RenderLoopConfig) -> Option<Self> {
        Self::start_with_config(render_config, None, None, UnchangedPolicyInRig::FromConfig)
    }

    /// То же, что [`Self::start`], но стартовый вид картинки передаётся бэкенду при сборке (как делает приложение).
    pub fn start_with_initial_look(
        ui_config: UiOverlayConfig,
        initial_look: Option<Arc<look_contract::LookSettings>>,
    ) -> Option<Self> {
        Self::start_with_initial_style(ui_config, initial_look, None)
    }

    /// То же, но передаёт бэкенду при сборке стартовую модель основы тона (как делает приложение, решение 195).
    pub fn start_with_initial_tone_base(
        ui_config: UiOverlayConfig,
        initial_tone_base: look_contract::ToneBaseModel,
    ) -> Option<Self> {
        Self::start_with_initial_style(ui_config, None, Some(initial_tone_base))
    }

    fn start_with_initial_style(
        ui_config: UiOverlayConfig,
        initial_look: Option<Arc<look_contract::LookSettings>>,
        initial_tone_base: Option<look_contract::ToneBaseModel>,
    ) -> Option<Self> {
        let render_config = RenderLoopConfig {
            ui_overlay: ui_config,
            ..RenderLoopConfig::default()
        };
        Self::start_with_config(
            render_config,
            initial_look,
            initial_tone_base,
            UnchangedPolicyInRig::EveryTickDrawn,
        )
    }

    fn start_with_config(
        render_config: RenderLoopConfig,
        initial_look: Option<Arc<look_contract::LookSettings>>,
        initial_tone_base: Option<look_contract::ToneBaseModel>,
        policy: UnchangedPolicyInRig,
    ) -> Option<Self> {
        // Один кадр камеры отдаётся на первый же vsync (политика буфера с запасом ждала бы второй).
        let stream = RawFrameStream::new(RawFrameStreamConfig {
            viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
            ..RawFrameStreamConfig::default()
        });
        let log = Arc::new(EventLog::default());
        let alive = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (factory_log, factory_alive) = (Arc::clone(&log), Arc::clone(&alive));
        let white = held_white(4000.0);
        let make_backend = match policy {
            UnchangedPolicyInRig::EveryTickDrawn => headless_camera_backend_with_white_or_skip,
            UnchangedPolicyInRig::FromConfig => headless_camera_backend_keeping_unchanged_policy_or_skip,
        };
        let backend = make_backend(
            &render_config,
            stream.viewfinder(),
            Box::new(move |context| {
                Ok(Box::new(TestImporter {
                    context: context.clone(),
                    log: Arc::clone(&factory_log),
                    alive: Arc::clone(&factory_alive),
                    entries: HashMap::new(),
                    counters: ImportCacheCounters::default(),
                }))
            }),
            white,
        )?;
        let backend = match initial_look {
            Some(look) => backend.with_initial_look(look),
            None => backend,
        };
        let backend = match initial_tone_base {
            Some(model) => backend.with_initial_tone_base(model),
            None => backend,
        };
        let baseline = backend.context().live_objects();
        let epoch = stream.producer().begin_stream(Arc::new(stream_passport())).epoch;
        Some(Self {
            backend,
            stream,
            epoch,
            log,
            book: Arc::new(ProducerBook::default()),
            journal: Arc::new(Journal::default()),
            baseline,
            frame_index: 0,
            sequence: 0,
        })
    }

    pub fn attach(&mut self, generation: u64, size: SurfaceSize) {
        self.backend
            .attach(fake_surface(&self.journal, generation), size)
            .expect("surface присоединяется");
    }

    pub fn detach(&mut self) {
        assert!(self.backend.detach().is_some(), "окно возвращено");
    }

    /// Новый кадр камеры. Метка сенсора и узор детерминированы номером кадра: два стенда видят одно и то же.
    pub fn offer_camera_frame(&mut self) {
        let sensor_ts = 1_000_000_000 + self.sequence * camera_rig::SENSOR_STEP_NANOS;
        let producer = self.stream.producer();
        let now = Instant::now();
        producer.offer_frame(
            self.epoch,
            OfferedFrame {
                sensor_timestamp: camera_contract::capture_result::SensorTimestamp::from_nanos(sensor_ts),
                acquire_fence: GpuWaitFence::already_signaled(),
                frame: Box::new(FakeFrame {
                    sensor_ts,
                    storage: Box::new(0),
                    log: Arc::clone(&self.log),
                    book: Arc::clone(&self.book),
                }),
            },
            now,
        );
        producer.offer_capture_result(self.epoch, result_for(sensor_ts, self.sequence), now);
        self.sequence += 1;
    }

    /// Один тик показа с любым исходом (пропуск тоже исход): возвращает исход бэкенда.
    pub fn tick(&mut self) -> PresentOutcome {
        let outcome = self.backend.render_and_present(timing(self.frame_index));
        self.frame_index += 1;
        outcome
    }

    /// Один кадр показа; возвращает его номер.
    pub fn present(&mut self) -> u64 {
        let index = self.frame_index;
        assert_presented_or_skipped(self.backend.render_and_present(timing(index)));
        self.frame_index += 1;
        index
    }

    pub fn viewfinder(&mut self) -> ViewfinderDiagnostics {
        self.backend.diagnostics().viewfinder.expect("диагностика видоискателя")
    }

    pub fn ui(&mut self) -> UiOverlayDiagnostics {
        self.viewfinder().ui_overlay
    }

    /// Предлагает новый кадр камеры и рисует кадры показа, пока ISP его не обработает (не дольше предела).
    /// Возвращает, сколько кадров показа ушло (первый из них — кадр с новым изображением).
    pub fn show_new_camera_frame(&mut self) -> u64 {
        let processed_before = self.viewfinder().counters.frames_processed;
        self.offer_camera_frame();
        for presented in 1..=MAX_FRAMES_TO_SETTLE {
            self.present();
            if self.viewfinder().counters.frames_processed > processed_before {
                return presented;
            }
        }
        panic!(
            "ISP не обработал новый кадр за {MAX_FRAMES_TO_SETTLE} кадров показа: {:?}",
            self.viewfinder()
        );
    }

    /// `count` кадров показа без нового кадра камеры: экран собирается из прежнего изображения.
    pub fn recompose(&mut self, count: u64) {
        let before = self.viewfinder().counters.recomposed;
        for _ in 0..count {
            self.present();
        }
        assert_eq!(
            self.viewfinder().counters.recomposed,
            before + count,
            "каждый кадр без нового кадра камеры — перекомпоновка"
        );
    }

    pub fn install(&mut self, probe: &Arc<SourceProbe>) {
        assert_eq!(
            self.backend.apply_ui_overlay_command(install_command(probe)),
            UiOverlayOutcome::Applied
        );
    }

    pub fn install_pictures(&mut self, probe: &Arc<PictureProbe>) {
        assert_eq!(
            self.backend.apply_ui_overlay_command(install_picture_command(probe)),
            UiOverlayOutcome::Applied
        );
    }

    pub fn live_objects(&self) -> gpu_vulkan::live_objects::LiveObjectSnapshot {
        self.backend.context().live_objects()
    }

    pub fn remove(&mut self) -> UiOverlayOutcome {
        self.backend.apply_ui_overlay_command(UiOverlayCommand::Remove)
    }

    pub fn live_object_total(&self) -> usize {
        self.backend.context().live_objects().total()
    }

    pub fn readback(&self) -> ViewfinderReadback {
        self.backend
            .read_viewfinder_image()
            .expect("readback изображения видоискателя")
            .expect("изображение видоискателя есть")
    }

    /// Остановка потока, отсоединение surface и проверки «ничего не утекло, validation чиста».
    pub fn finish(mut self) {
        self.stream.producer().end_stream(self.epoch);
        self.present();
        self.detach();
        let counters = self.stream.counters();
        assert_eq!(counters.leaked_leases, 0, "{counters:?}");
        assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
        assert_eq!(
            self.backend.context().live_objects(),
            self.baseline,
            "объекты Vulkan вернулись к исходным"
        );
        assert_no_validation_errors(self.backend.context());
    }
}
