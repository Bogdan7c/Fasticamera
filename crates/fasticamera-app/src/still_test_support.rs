//! Стенд тестов снимка приложения: НАСТОЯЩАЯ служба `photo-output` и настоящие сервис снимка, таблица и канал
//! экрана; подделаны только платформа (хранилище, кодеры, JPEG), часы, каталог камер и поток рендера.
//!
//! Рендер-подделка ([`FakeRenderPort`]) хранит планы и приёмники событий, которые получила, поэтому тест САМ
//! играет роль рендера: шлёт `Secured`/`PrimaryReadback`/`Failed` и читает, какую цель получил в ответ.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use android_shell::still_orphan_sweep::OrphanSweepReport;
use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::flash::FlashStillFailure;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::{CameraId, LensFacing};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use camera_contract::zoom::{ZoomCatalog, ZoomFraming, ZoomRatio};
use camera_screen::settings::{AspectRatio, FlashMode, RawCapture};
use camera_screen::shutter::{PrimaryFormat, ShotRequest, ShotRequestId};
use photo_output::dng::{DngError, DngInputs};
use photo_output::exif::{CameraDescription, CaptureMoment, ExifError, ExifInputs};
use photo_output::jpeg_exif::JpegExifError;
use photo_output::service::{CameraCatalog, StillFileFormats, WallClock};
use render_loop::still::{StillCommand, StillEventSink};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::CommandOutcome;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::still_media::{
    EncodeError, HeifEncodeSettings, HeifEncoderService, HeifSurfaceEncoder, JpegCompressor, MediaStoreError,
    MediaStoreService, PendingMedia, PendingMediaRequest, PublishedMedia, ShutterSoundService,
};
use still_contract::develop::{DevelopEvent, PrimaryTarget};
use still_contract::frame_choice::FrameChoice;
use still_contract::plan::{StillId, StillOutputs, StillPlan};
use still_contract::shot::{AsShotNeutral, DigitalGainEv, OwnedPixels, Rgba8Image, StillFrameFacts, StillRotation};

use crate::camera_screen_capture::CaptureService;
use crate::camera_screen_input::{ScreenInput, ScreenInputReceiver, screen_input_channel};
use crate::capture_backend::CaptureBackend;
use crate::flash_camera_port::FlashCameraPort;
use crate::flash_still_relay::FlashStillRelay;
use crate::flash_test_support::FakeFlashCamera;
use crate::photo_config::PhotoConfig;
use crate::still_render_port::{StillCommandDelivery, StillRenderPort};
use crate::still_runtime::{OrphanSweepSource, StillRuntime, StillRuntimeDeps};

/// Предел ожидания наблюдаемого состояния: достаточно для рабочего потока, мало для зависшего теста.
pub const WAIT: Duration = Duration::from_secs(5);

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Ждёт, пока условие станет истинным, и падает с понятным текстом по сроку. Не `sleep` «на удачу»: условие
/// читает наблюдаемое состояние, пауза лишь не даёт крутить процессор.
pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Запрос снимка экрана с номером `request`: JPEG (путь без `NativeSurface`), RAW выключен.
pub fn shot(request: u64) -> ShotRequest {
    ShotRequest {
        request: ShotRequestId(request),
        pressed_at: MonotonicNanos::from_nanos(7_000_000),
        framing: ZoomFraming {
            aspect: AspectRatio::FourByThree.crop_aspect(),
            facing: LensFacing::Back,
            ratio: ZoomRatio::ONE,
            catalog: Arc::new(ZoomCatalog::default()),
        },
        orientation: DeviceOrientation::LandscapeLeft,
        primary_format: PrimaryFormat::Jpeg,
        flash: FlashMode::Off,
        raw: RawCapture::Off,
        aspect: AspectRatio::FourByThree,
    }
}

// ---- Поток рендера ------------------------------------------------------------------------------------------

/// Как рендер-подделка отвечает на команды.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Accepting,
    Gone,
    Unavailable,
}

/// Вид команды снимка в журнале рендера-подделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggedCommand {
    Begin,
    Target,
    FlashFailed,
}

/// Что рендер-подделка получила.
pub struct FakeRenderPort {
    mode: Mutex<RenderMode>,
    plans: Mutex<Vec<StillPlan>>,
    sinks: Mutex<Vec<(StillId, Arc<dyn StillEventSink>)>>,
    /// Цели основной картинки: номер снимка и «это чтение в память».
    targets: Mutex<Vec<(StillId, bool)>>,
    unavailable_offers: Mutex<Vec<StillId>>,
    /// Сообщения «снимок со вспышкой не снят» (14d): номер снимка и причина.
    flash_failures: Mutex<Vec<(StillId, FlashStillFailure)>>,
    /// Порядок принятых команд: нужен, чтобы проверить «`Begin` раньше любой вспышечной команды».
    command_log: Mutex<Vec<(LoggedCommand, StillId)>>,
}

impl FakeRenderPort {
    pub fn new(mode: RenderMode) -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(mode),
            plans: Mutex::default(),
            sinks: Mutex::default(),
            targets: Mutex::default(),
            unavailable_offers: Mutex::default(),
            flash_failures: Mutex::default(),
            command_log: Mutex::default(),
        })
    }

    /// Причины срыва снимков со вспышкой, о которых приложение сообщило рендеру.
    pub fn flash_failures(&self) -> Vec<(StillId, FlashStillFailure)> {
        lock(&self.flash_failures).clone()
    }

    /// Принятые команды по порядку: вид и номер снимка.
    pub fn command_log(&self) -> Vec<(LoggedCommand, StillId)> {
        lock(&self.command_log).clone()
    }

    pub fn plans(&self) -> Vec<StillPlan> {
        lock(&self.plans).clone()
    }

    /// Рендер «выдаёт событие» в приёмник снимка `id` (так делает поток рендера).
    pub fn emit(&self, id: StillId, event: DevelopEvent) {
        let sink = lock(&self.sinks)
            .iter()
            .find(|(sink_id, _)| *sink_id == id)
            .map(|(_, sink)| Arc::clone(sink))
            .expect("рендер не получал Begin для этого снимка");
        sink.deliver(event);
    }

    /// Ждёт цель основной картинки для снимка и говорит, что это чтение в память.
    pub fn wait_readback_target(&self, id: StillId) {
        wait_until(
            "цель основной картинки (чтение в память)",
            || {
                lock(&self.targets)
                    .iter()
                    .any(|(target_id, readback)| *target_id == id && *readback)
            },
        );
    }

    pub fn unavailable_offers(&self) -> Vec<StillId> {
        lock(&self.unavailable_offers).clone()
    }
}

impl StillRenderPort for FakeRenderPort {
    fn send_still_command(&self, command: StillCommand) -> StillCommandDelivery {
        match *lock(&self.mode) {
            RenderMode::Gone => return StillCommandDelivery::RenderGone,
            RenderMode::Unavailable => {
                return StillCommandDelivery::RenderUnavailable {
                    reason: "тест: рендера нет".to_owned(),
                };
            }
            RenderMode::Accepting => {}
        }
        let kind = match &command {
            StillCommand::Begin(_) => LoggedCommand::Begin,
            StillCommand::Target(_) => LoggedCommand::Target,
            StillCommand::FlashFailed { .. } => LoggedCommand::FlashFailed,
        };
        lock(&self.command_log).push((kind, command.still_id()));
        match command {
            StillCommand::FlashFailed { id, failure } => lock(&self.flash_failures).push((id, failure)),
            StillCommand::Begin(begin) => {
                lock(&self.plans).push(begin.plan.clone());
                lock(&self.sinks).push((begin.plan.id, begin.events));
            }
            StillCommand::Target(offer) => match offer {
                still_contract::develop::PrimaryTargetOffer::Ready { id, target } => {
                    lock(&self.targets).push((id, matches!(target, PrimaryTarget::Readback)));
                }
                still_contract::develop::PrimaryTargetOffer::Unavailable { id } => {
                    lock(&self.unavailable_offers).push(id);
                }
            },
        }
        StillCommandDelivery::Delivered
    }
}

// ---- Платформа ----------------------------------------------------------------------------------------------

/// Состояние черновика в подделке хранилища.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftState {
    Pending,
    Published,
    Discarded,
}

#[derive(Debug, Clone)]
pub struct DraftRecord {
    pub name: String,
    pub bytes: Vec<u8>,
    pub state: DraftState,
}

/// Хранилище-подделка. `block_creation` задерживает создание черновика: так тест держит рабочий поток службы
/// занятым и наполняет её очередь событий.
#[derive(Default)]
pub struct FakeStore {
    drafts: Arc<Mutex<Vec<DraftRecord>>>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    /// Сколько раз служба вошла в `create_pending` (в том числе и ждущих у закрытых ворот).
    entered: AtomicU64,
}

impl FakeStore {
    pub fn drafts(&self) -> Vec<DraftRecord> {
        lock(&self.drafts).clone()
    }

    pub fn creation_attempts(&self) -> u64 {
        self.entered.load(Ordering::SeqCst)
    }

    pub fn block_creation(&self) {
        *lock(&self.gate.0) = true;
    }

    pub fn unblock_creation(&self) {
        *lock(&self.gate.0) = false;
        self.gate.1.notify_all();
    }
}

impl MediaStoreService for FakeStore {
    fn create_pending(&self, request: &PendingMediaRequest) -> Result<Box<dyn PendingMedia>, MediaStoreError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        {
            let mut blocked = lock(&self.gate.0);
            while *blocked {
                blocked = self.gate.1.wait(blocked).unwrap_or_else(PoisonError::into_inner);
            }
        }
        let mut drafts = lock(&self.drafts);
        let index = drafts.len();
        drafts.push(DraftRecord {
            name: request.display_name.clone(),
            bytes: Vec::new(),
            state: DraftState::Pending,
        });
        Ok(Box::new(FakeDraft {
            index,
            uri: format!("content://fake/{}", request.display_name),
            drafts: Arc::clone(&self.drafts),
        }))
    }
}

struct FakeDraft {
    index: usize,
    uri: String,
    drafts: Arc<Mutex<Vec<DraftRecord>>>,
}

impl FakeDraft {
    fn set_state(&self, state: DraftState) {
        if let Some(draft) = lock(&self.drafts).get_mut(self.index) {
            draft.state = state;
        }
    }
}

impl PendingMedia for FakeDraft {
    fn uri(&self) -> &str {
        &self.uri
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), MediaStoreError> {
        if let Some(draft) = lock(&self.drafts).get_mut(self.index) {
            draft.bytes.extend_from_slice(bytes);
        }
        Ok(())
    }

    fn publish(self: Box<Self>) -> Result<PublishedMedia, MediaStoreError> {
        self.set_state(DraftState::Published);
        Ok(PublishedMedia { uri: self.uri.clone() })
    }

    fn discard(self: Box<Self>) {
        self.set_state(DraftState::Discarded);
    }
}

/// HEIF в этих тестах не нужен (идём путём JPEG): любая попытка открыть кодер — отказ.
pub struct RefusingHeif;

impl HeifEncoderService for RefusingHeif {
    fn open(
        &self,
        _target: &dyn PendingMedia,
        _settings: &HeifEncodeSettings,
    ) -> Result<Box<dyn HeifSurfaceEncoder>, EncodeError> {
        Err(EncodeError::NotAttached)
    }
}

/// JPEG-подделка: узнаваемое тело.
pub struct FakeJpeg;

impl FakeJpeg {
    pub const BODY: [u8; 7] = [0xFF, 0xD8, b'J', b'P', b'G', 0xFF, 0xD9];
}

impl JpegCompressor for FakeJpeg {
    fn compress(&self, _rgba: &[u8], _width: u32, _height: u32, _quality: u8) -> Result<Vec<u8>, EncodeError> {
        Ok(Self::BODY.to_vec())
    }
}

/// Форматы-подделка: блок Exif и вставка узнаваемы, DNG — заголовок TIFF.
pub struct FakeFormats;

impl StillFileFormats for FakeFormats {
    fn write_dng(&self, _inputs: &DngInputs<'_>, out: &mut dyn Write) -> Result<(), DngError> {
        out.write_all(b"II*\0FAKE-DNG")?;
        Ok(())
    }

    fn build_exif(&self, inputs: &ExifInputs<'_>) -> Result<Vec<u8>, ExifError> {
        Ok(format!("EXIF:{}", inputs.camera.model).into_bytes())
    }

    fn insert_jpeg_exif(&self, jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>, JpegExifError> {
        let mut result = exif_tiff.to_vec();
        result.extend_from_slice(jpeg);
        Ok(result)
    }
}

pub struct FixedClock;

impl WallClock for FixedClock {
    fn now(&self) -> CaptureMoment {
        CaptureMoment {
            utc_millis: 1_700_000_000_000,
            utc_offset_minutes: 180,
        }
    }
}

/// Каталог камер: знает любую камеру (отдельные тесты проверяют настоящий каталог).
pub struct AnyCameraCatalog;

impl CameraCatalog for AnyCameraCatalog {
    fn describe_camera(&self, _camera: &CameraId) -> Option<CameraDescription> {
        Some(CameraDescription {
            make: "OnePlus".to_owned(),
            model: "PHB110".to_owned(),
            unique_camera_model: "OnePlus PHB110 wide".to_owned(),
            software: "Fasticamera test".to_owned(),
            lens: None,
        })
    }
}

#[derive(Default)]
pub struct CountingSound {
    pub calls: Mutex<u64>,
}

impl ShutterSoundService for CountingSound {
    fn play_shutter_sound(&self) -> CommandOutcome {
        *lock(&self.calls) += 1;
        CommandOutcome::Queued
    }
}

pub struct FixedSweep(pub OrphanSweepReport);

impl OrphanSweepSource for FixedSweep {
    fn orphan_sweep_report(&self) -> OrphanSweepReport {
        self.0.clone()
    }
}

// ---- Кадр рендера -------------------------------------------------------------------------------------------

pub const PRIMARY_SIZE: PixelSize = PixelSize::new(8, 6);
const RAW_SIZE: PixelSize = PixelSize::new(16, 12);

/// Факты снятого кадра, какие рендер шлёт событием `Secured` (JPEG, без DNG).
pub fn facts_for(id: StillId) -> Box<StillFrameFacts> {
    let passport = StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw10,
        buffer_size: RAW_SIZE,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: RAW_SIZE,
        },
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с допустимо"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: Vec::new(),
    };
    let timestamp = SensorTimestamp::from_nanos(1_000_000);
    Box::new(StillFrameFacts {
        id,
        outputs: StillOutputs {
            primary: still_contract::plan::PrimaryFormat::Jpeg,
            raw: still_contract::plan::RawOutput::Skip,
        },
        stream: Arc::new(passport),
        sensor_timestamp: timestamp,
        capture: FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(7),
                sensor_timestamp: Some(timestamp),
                exposure_time: None,
                sensitivity_iso: Some(100),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints::default(),
        },
        choice: FrameChoice::ShownAtPress {
            sensor_timestamp: timestamp,
            shown_at: MonotonicNanos::from_nanos(5),
        },
        crop: PixelRect {
            left: 0,
            top: 0,
            size: PRIMARY_SIZE,
        },
        rotation: StillRotation::Clockwise90,
        neutral: AsShotNeutral([0.5, 1.0, 0.6]),
        digital_gain: DigitalGainEv(0.0),
    })
}

/// Картинка JPEG-пути.
pub fn readback_image() -> Rgba8Image {
    let bytes = vec![9_u8; usize::try_from(PRIMARY_SIZE.pixel_count() * 4).expect("влезает")];
    Rgba8Image::new(PRIMARY_SIZE, Arc::new(OwnedPixels(bytes))).expect("размер совпал")
}

// ---- Стенд --------------------------------------------------------------------------------------------------

/// Поднятая настоящая служба, сервис снимка и экран (канал входа) вокруг подделок.
pub struct StillRig {
    pub render: Arc<FakeRenderPort>,
    pub store: Arc<FakeStore>,
    pub sound: Arc<CountingSound>,
    pub backend: CaptureBackend,
    pub service: photo_output::service::StillService,
    pub capture: Arc<dyn CaptureService>,
    pub screen: ScreenInputReceiver,
    /// Камера-подделка для вспышки (14d) и мост её итогов к рендеру.
    pub flash_camera: Arc<FakeFlashCamera>,
    pub flash_relay: Arc<FlashStillRelay>,
}

/// Поднятая настоящая служба с подделками вокруг неё, без экрана: экран подключает тот, кому он нужен (стенд
/// `StillRig` цепляет свой канал, тесты маршрутизатора — настоящий `CameraScreenService`).
pub struct StartedStill {
    pub store: Arc<FakeStore>,
    pub sound: Arc<CountingSound>,
    pub service: photo_output::service::StillService,
    pub backend: CaptureBackend,
}

/// Запускает настоящую службу снимков вокруг `render` и подделок платформы.
pub fn start_still_runtime(config: &PhotoConfig, render: &Arc<FakeRenderPort>) -> StartedStill {
    start_still_runtime_with_flash(config, render, FakeFlashCamera::new(), Arc::default())
}

/// То же, но с заданной камерой для вспышки и мостом её итогов (настоящий движок или подделка).
pub fn start_still_runtime_with_flash(
    config: &PhotoConfig,
    render: &Arc<FakeRenderPort>,
    flash_camera: Arc<dyn FlashCameraPort>,
    flash_relay: Arc<FlashStillRelay>,
) -> StartedStill {
    let store = Arc::new(FakeStore::default());
    let sound = Arc::new(CountingSound::default());
    let deps = StillRuntimeDeps {
        store: store.clone(),
        heif: Arc::new(RefusingHeif),
        jpeg: Arc::new(FakeJpeg),
        formats: Arc::new(FakeFormats),
        clock: Arc::new(FixedClock),
        cameras: Arc::new(AnyCameraCatalog),
        render: render.clone(),
        shutter_sound: sound.clone(),
        orphan_sweep: Arc::new(FixedSweep(OrphanSweepReport::Removed { count: 0 })),
        flash_camera,
        flash_relay,
    };
    let runtime = StillRuntime::start(config, deps).expect("служба запускается");
    let (service, backend) = runtime.into_parts();
    StartedStill {
        store,
        sound,
        service,
        backend,
    }
}

impl StillRig {
    pub fn start() -> Self {
        Self::start_with(PhotoConfig::default(), RenderMode::Accepting)
    }

    pub fn start_with(config: PhotoConfig, render_mode: RenderMode) -> Self {
        let render = FakeRenderPort::new(render_mode);
        let flash_camera = FakeFlashCamera::new();
        let flash_relay = Arc::new(FlashStillRelay::default());
        let StartedStill {
            store,
            sound,
            service,
            backend,
        } = start_still_runtime_with_flash(&config, &render, flash_camera.clone(), flash_relay.clone());
        let capture = backend.service();
        let (sender, screen) = screen_input_channel(64);
        backend.completion_slot().attach(sender.injector());
        Self {
            render,
            store,
            sound,
            backend,
            service,
            capture,
            screen,
            flash_camera,
            flash_relay,
        }
    }

    /// Счётчики сервиса снимка (настоящая служба: режим `real`).
    pub fn capture_counters(&self) -> crate::still_capture_service::CaptureCounters {
        self.real_diagnostics().capture
    }

    /// Маршруты принятых снимков (со вспышкой или без).
    pub fn capture_flash_routes(&self) -> Vec<crate::still_capture_service::FlashRouteEntry> {
        self.backend.flash_routes()
    }

    fn real_diagnostics(&self) -> crate::capture_backend::RealDiagnostics {
        self.backend.diagnostics().real.expect("стенд поднят настоящим режимом")
    }

    /// Ставит новый экран: у него свой канал и эпоха (как переустановка экрана в приложении).
    pub fn reinstall_screen(&mut self) {
        let (sender, screen) = screen_input_channel(64);
        self.backend.completion_slot().attach(sender.injector());
        self.screen = screen;
    }

    /// Всё, что экран получил к этому моменту.
    pub fn screen_inputs(&mut self) -> Vec<ScreenInput> {
        std::iter::from_fn(|| self.screen.try_next()).collect()
    }

    /// Ждёт, пока экран получит `count` сообщений, и отдаёт их.
    pub fn wait_screen_inputs(&mut self, count: usize) -> Vec<ScreenInput> {
        let mut received = Vec::new();
        let deadline = Instant::now() + WAIT;
        while received.len() < count {
            received.extend(self.screen_inputs());
            assert!(Instant::now() < deadline, "экран получил {received:?}, ждали {count}");
            std::thread::sleep(Duration::from_millis(2));
        }
        received
    }
}
