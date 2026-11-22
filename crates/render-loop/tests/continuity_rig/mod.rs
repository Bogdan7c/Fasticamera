//! Общий стенд сценариев непрерывности (сессия 13): настоящий бэкенд показа на реальном драйвере (RADV,
//! headless, полная validation), настоящий `RawFrameStream` с управляемым синтетическим RAW, однородные
//! кадры-сцены с эталоном `isp-model` и запись того, что рендер сообщил UI о фоне. Сценарии — в
//! `vulkan_continuity*.rs`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use super::support::{Journal, fake_surface};
use super::vulkan_support::{assert_no_validation_errors, headless_camera_backend_with_directives_or_skip};
use camera_contract::capabilities::{LensShading, Rational, RawSupport, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{DirectiveSlot, ThreeAProfile, WhiteBalanceDirective, WhiteBalanceMode};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::sync::GpuWaitFence;
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::{CameraNeutral, DngColorProfile};
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference;
use isp_model::sensor::SensorDescription;
use isp_model::tone::ToneCurveSettings;
use raw_frame_stream::{
    OfferedFrame, ProducerFrame, RawFrameLease, RawFrameStream, RawFrameStreamConfig, ReleaseFences, StreamEpoch,
};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentOutcome, PresentTarget};
use render_loop::config::{
    ContinuityConfig, DisplayColorConfig, RenderLoopConfig, RevealExposureCheck, ViewfinderConfig,
};
use render_loop::ui_overlay::UiOverlayCommand;
use render_loop::viewfinder::{
    AutoExposureLinks, FrameLabel, FrameRecord, ViewfinderCommand, ViewfinderCommandOutcome,
};
use render_loop::vulkan_backend::{
    CameraFrameImporter, CameraImportError, SubmissionSerial, ViewfinderReadback, VulkanDisplayBackend,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::SurfaceSize;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{BackdropContinuity, UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::light::LightDirection;

pub const SIZE_A: PixelSize = PixelSize::new(64, 48);
/// Другой размер буфера, как у 4000×3000 против 4096×3072: выходное изображение у потоков разное.
pub const SIZE_B: PixelSize = PixelSize::new(80, 60);
pub const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
pub const BLACK_LEVEL: u16 = 64;
pub const WHITE_LEVEL: u16 = 1023;
pub const MANUAL_KELVIN: f64 = 4000.0;
pub const TOLERANCE: f32 = 3.0 / 1023.0;
/// Сколько кадров показа ждать, пока кадр дойдёт до экрана.
pub const MAX_TICKS: usize = 40;
pub const MAIN: &str = "main";
pub const WIDE: &str = "wide";

// ---- сцена: однородные кадры, эталон isp-model ----

#[derive(Debug, Clone, Copy)]
pub struct CellValues {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
}

/// Серый под нейтраль ручной белой точки: кадр потока A.
pub fn gray() -> CellValues {
    let neutral = scene_neutral();
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL) * 0.4;
    let channel = |weight: f64| BLACK_LEVEL + (span * weight).round() as u16;
    CellValues {
        red: channel(neutral[0]),
        green: channel(neutral[1]),
        blue: channel(neutral[2]),
    }
}

/// Насыщенно-красный: кадр потока B (заметно отличается от серого).
pub fn reddish() -> CellValues {
    CellValues {
        red: 600,
        green: 200,
        blue: 120,
    }
}

/// Очень тёмная сцена: наш AE не найдёт показанный уровень правдоподобным (цель далеко).
pub fn dark() -> CellValues {
    CellValues {
        red: 70,
        green: 70,
        blue: 70,
    }
}

pub fn calibration() -> Vec<camera_contract::capabilities::ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

pub fn scene_neutral() -> [f64; 3] {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль цвета");
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(MANUAL_KELVIN),
        duv: Duv(0.0),
    })
    .expect("точка на кривой Планка");
    profile.neutral_for_white(white).expect("нейтраль").values()
}

pub fn manual_white_directive() -> WhiteBalanceDirective {
    WhiteBalanceDirective {
        mode: WhiteBalanceMode::Manual {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(MANUAL_KELVIN).expect("температура"),
                tint: Tint::NEUTRAL,
            },
        },
        profile: ThreeAProfile::Photo,
    }
}

pub fn bggr_sample(cell: CellValues, x: u32, y: u32) -> u16 {
    match (x % 2, y % 2) {
        (0, 0) => cell.blue,
        (1, 1) => cell.red,
        _ => cell.green,
    }
}

pub fn passport(camera: &str, size: PixelSize) -> StreamPassport {
    StreamPassport {
        camera: camera_id(camera),
        raw_format: RawFormat::Raw16,
        buffer_size: size,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect { left: 0, top: 0, size },
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [u32::from(BLACK_LEVEL); 4],
            white_level: u32::from(WHITE_LEVEL),
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: calibration(),
    }
}

/// Подсказки HAL нарочно неверные: ISP обязан их игнорировать.
pub fn capture_hints() -> CaptureColorHints {
    let rational = |numerator| Rational {
        numerator,
        denominator: 1000,
    };
    CaptureColorHints {
        neutral_color_point: Some([rational(900), rational(1000), rational(200)]),
        color_gains: Some(camera_contract::frame_passport::ColorCorrectionGains([
            0.3, 1.0, 1.0, 7.0,
        ])),
        ..CaptureColorHints::default()
    }
}

/// Эталон `isp-model`: цвет пикселя выхода для однородной ячейки `cell` потока `passport`.
pub fn reference_color(passport: &StreamPassport, cell: CellValues) -> [f32; 3] {
    let sensor = SensorDescription::from_stream_passport(passport, &passport.color_calibration).expect("сенсор");
    let frame = FrameInputs::from_capture_hints(&sensor, &capture_hints()).expect("уровни");
    let neutral = CameraNeutral::new(scene_neutral()).expect("нейтраль");
    let controls = ProcessingControls {
        tone: ToneCurveSettings::default(),
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Preview, DisplayGamut::Srgb)
    };
    let params = IspParams::resolve(&sensor, &frame, &controls).expect("параметры");
    let samples = (0..16).map(|index| bggr_sample(cell, index % 4, index / 4)).collect();
    let raw = RawMosaic::new(PixelSize::new(4, 4), BayerPattern::BGGR, samples).expect("мозаика");
    run_reference(&raw, &params)
        .expect("эталон")
        .display_encoded
        .pixel(0, 0)
}

// ---- поддельный производитель и импортёр ----

#[derive(Default)]
pub struct Returned(Mutex<Vec<i64>>);

pub struct FakeFrame {
    pub sensor_ts: i64,
    pub storage: Box<u8>,
    pub returned: Arc<Returned>,
}

// SAFETY: указатель из `hardware_buffer` указывает на `storage`, живой до `return_to_producer`.
unsafe impl ProducerFrame for FakeFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        let pointer = NonNull::from(&*self.storage).cast::<c_void>();
        // SAFETY: `storage` живёт, пока жив `self`; заимствование ограничено `&self`.
        unsafe { BorrowedHardwareBuffer::from_raw(pointer) }
    }

    fn return_to_producer(self: Box<Self>, _release: ReleaseFences) {
        self.returned
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.sensor_ts);
    }
}

pub type Scene = Arc<Mutex<HashMap<i64, CellValues>>>;

/// Импортёр: каждый кадр — однородная ячейка по метке сенсора, размер — из паспорта потока кадра.
pub struct SceneImporter {
    pub context: GpuContext,
    pub scene: Scene,
    pub entries: HashMap<i64, (SyntheticRaw16, u64)>,
    pub counters: ImportCacheCounters,
}

impl CameraFrameImporter for SceneImporter {
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError> {
        let sensor_ts = lease.passport().sensor_timestamp.as_nanos();
        if let Some((source, last_use)) = self.entries.get_mut(&sensor_ts) {
            *last_use = submission.0;
            self.counters.hits += 1;
            return Ok(source.source());
        }
        self.counters.misses += 1;
        let cell = *self
            .scene
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&sensor_ts)
            .ok_or_else(|| CameraImportError::Platform(format!("неизвестный кадр {sensor_ts}")))?;
        let size = lease.passport().stream.buffer_size;
        let texels: Vec<u16> = (0..size.height)
            .flat_map(|y| (0..size.width).map(move |x| bggr_sample(cell, x, y)))
            .collect();
        let source = SyntheticRaw16::new(&self.context, size.width, size.height, &texels)?;
        let mosaic = source.source();
        self.entries.insert(sensor_ts, (source, submission.0));
        Ok(mosaic)
    }

    fn retire_completed(&mut self, completed: SubmissionSerial) {
        self.entries.retain(|_, (_, last_use)| *last_use > completed.0);
    }

    fn release_all_imports_gpu_idle(&mut self) {
        self.entries.clear();
    }

    fn imports_alive(&self) -> usize {
        self.entries.len()
    }

    fn cache_counters(&self) -> ImportCacheCounters {
        self.counters
    }
}

// ---- источник UI: пустой список, но запоминает, что рендер сообщил о фоне ----

pub struct BackdropRecorder {
    pub seen: Arc<Mutex<Vec<BackdropContinuity>>>,
}

impl UiFrameSource for BackdropRecorder {
    fn name(&self) -> &'static str {
        "continuity-test"
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request.backdrop);
        Ok(DrawList::new(LightDirection::from_angle_degrees(45.0)))
    }
}

// ---- стенд ----

/// Что видно после одного кадра показа.
pub struct Tick {
    pub record: FrameRecord,
    /// Фон, о котором рендер сообщил UI в этом кадре.
    pub ui_backdrop: BackdropContinuity,
    pub holding: bool,
    /// Удержанная картинка в момент этого кадра (читается только в переходных кадрах: потом её уже нет).
    pub held: Option<ViewfinderReadback>,
}

pub struct Rig {
    pub backend: VulkanDisplayBackend,
    pub stream: RawFrameStream,
    pub scene: Scene,
    pub returned: Arc<Returned>,
    pub backdrops: Arc<Mutex<Vec<BackdropContinuity>>>,
    /// Объекты GPU до присоединения surface: после отсоединения их должно быть ровно столько же.
    pub baseline: gpu_vulkan::live_objects::LiveObjectSnapshot,
    pub next_tick: u64,
    pub next_sensor_ts: i64,
    pub next_sequence: i64,
    pub with_exposure: bool,
    /// Ящик указаний нашему AE (пишет «движок» теста): без AE пустой и ни на что не влияет.
    pub exposure_directives: DirectiveSlot<camera_contract::auto_exposure::ExposureDirective>,
    pub _journal: Arc<Journal>,
}

pub fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

/// Сроки теста в мс: кадры идут по 1 мс времени показа, поэтому сроки мелкие.
pub fn test_continuity(reveal_exposure: RevealExposureCheck) -> ContinuityConfig {
    ContinuityConfig {
        max_hold_ms: 25,
        max_hold_without_stream_ms: 60,
        reveal_exposure,
        ..ContinuityConfig::default()
    }
}

/// Снимки в стенде (сессия 14): настройки движка снимков и размер кольца камеры.
#[derive(Debug, Clone, Copy)]
pub struct StillSetup {
    pub config: render_loop::config::StillDevelopConfig,
    /// Потолок кадров кольца снимков (0 — кольцо выключено).
    pub ring_frames: u32,
}

impl Rig {
    pub fn new(continuity: ContinuityConfig, with_auto_exposure: bool) -> Option<Self> {
        // Кольцо снимков выключено: стенды непрерывности про видоискатель (кольцо держало бы доли кадров серий и
        // хаб возвращал бы кадры позже; поведение кольца проверяют `vulkan_still_*.rs`).
        Self::with_stills(continuity, with_auto_exposure, None)
    }

    /// Стенд со снимками (`Some`) или без них (`None`: кольцо выключено, как в стендах видоискателя).
    pub fn with_stills(
        continuity: ContinuityConfig,
        with_auto_exposure: bool,
        stills: Option<StillSetup>,
    ) -> Option<Self> {
        let render_config = RenderLoopConfig {
            display_color: DisplayColorConfig::SrgbOnly,
            viewfinder: ViewfinderConfig {
                continuity,
                ..ViewfinderConfig::default()
            },
            still: stills.map_or_else(Default::default, |setup| setup.config),
            ..RenderLoopConfig::default()
        };
        render_config.validate().expect("конфиг корректен");
        let stream = RawFrameStream::new(RawFrameStreamConfig {
            viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
            snapshot_ring_max_frames: stills.map_or(0, |setup| setup.ring_frames),
            ..RawFrameStreamConfig::default()
        });
        let scene: Scene = Arc::default();
        let importer_scene = Arc::clone(&scene);
        let directives = DirectiveSlot::new();
        directives.publish(manual_white_directive());
        let backend = headless_camera_backend_with_directives_or_skip(
            &render_config,
            stream.viewfinder(),
            Box::new(move |context| {
                Ok(Box::new(SceneImporter {
                    context: context.clone(),
                    scene: Arc::clone(&importer_scene),
                    entries: HashMap::new(),
                    counters: ImportCacheCounters::default(),
                }))
            }),
            directives,
        )?;
        let backend = if stills.is_some() {
            backend.with_still_capture(stream.snapshot_ring())
        } else {
            backend
        };
        let exposure_directives = DirectiveSlot::new();
        let mut backend = if with_auto_exposure {
            backend.with_auto_exposure(AutoExposureLinks {
                directives: exposure_directives.clone(),
                proposals: camera_contract::auto_exposure::ExposureProposalChannel::new(),
            })
        } else {
            backend
        };
        let baseline = backend.context().live_objects();
        let journal = Arc::new(Journal::default());
        backend
            .attach(fake_surface(&journal, 1), SURFACE_SIZE)
            .expect("surface присоединяется");
        let backdrops: Arc<Mutex<Vec<BackdropContinuity>>> = Arc::default();
        backend.apply_ui_overlay_command(UiOverlayCommand::Install(Box::new(BackdropRecorder {
            seen: Arc::clone(&backdrops),
        })));
        Some(Self {
            backend,
            stream,
            scene,
            returned: Arc::default(),
            backdrops,
            baseline,
            next_tick: 0,
            next_sensor_ts: 1_000_000_000,
            next_sequence: 1,
            with_exposure: with_auto_exposure,
            exposure_directives,
            _journal: journal,
        })
    }

    pub fn begin(&self, passport: &Arc<StreamPassport>) -> StreamEpoch {
        self.stream.producer().begin_stream(Arc::clone(passport)).epoch
    }

    /// Предлагает кадр потока `epoch` с однородной ячейкой `cell` и метаданными результата съёмки.
    pub fn offer(&mut self, epoch: StreamEpoch, cell: CellValues) {
        let (sensor_ts, sequence) = self.offer_frame_only(epoch, cell);
        self.offer_capture_metadata(epoch, sensor_ts, sequence);
    }

    /// Предлагает кадр БЕЗ метаданных результата съёмки: кадр приходит, но параметров ISP для него нет
    /// (`NoMetadataYet`) и показать его нельзя. Так в тесте поток «стримит, но не показывается».
    pub fn offer_without_metadata(&mut self, epoch: StreamEpoch, cell: CellValues) {
        self.offer_frame_only(epoch, cell);
    }

    /// Кадр в очередь потока; возвращает метку сенсора и номер кадра для метаданных.
    fn offer_frame_only(&mut self, epoch: StreamEpoch, cell: CellValues) -> (i64, i64) {
        let sensor_ts = self.next_sensor_ts;
        let sequence = self.next_sequence;
        self.next_sensor_ts += 16_666_667;
        self.next_sequence += 1;
        self.scene
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(sensor_ts, cell);
        self.stream.producer().offer_frame(
            epoch,
            OfferedFrame {
                sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
                acquire_fence: GpuWaitFence::already_signaled(),
                frame: Box::new(FakeFrame {
                    sensor_ts,
                    storage: Box::new(0),
                    returned: Arc::clone(&self.returned),
                }),
            },
            Instant::now(),
        );
        (sensor_ts, sequence)
    }

    /// Результат съёмки кадра (парные выдержка и ISO нужны нашему AE для замера; без AE ни на что не влияют).
    fn offer_capture_metadata(&mut self, epoch: StreamEpoch, sensor_ts: i64, sequence: i64) {
        self.stream.producer().offer_capture_result(
            epoch,
            FrameCaptureDetails {
                result: CaptureResultMeta {
                    frame_number: FrameNumber(sequence),
                    sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
                    exposure_time: self.with_exposure.then(|| std::time::Duration::from_millis(10)),
                    sensitivity_iso: Some(100),
                    reported_frame_duration: None,
                    rolling_shutter_skew: None,
                    three_a: camera_contract::three_a::ThreeAResultState::unreported(
                        camera_contract::three_a::RequestOrigin::Repeating,
                    ),
                },
                color: capture_hints(),
            },
            Instant::now(),
        );
    }

    /// Один кадр показа; возвращает метку/кадрирование последнего принятого кадра и фон под UI.
    pub fn tick(&mut self) -> Tick {
        assert_eq!(
            self.backend.render_and_present(timing(self.next_tick)),
            PresentOutcome::Presented
        );
        self.next_tick += 1;
        let viewfinder = self.backend.diagnostics().viewfinder.expect("диагностика видоискателя");
        let record = viewfinder
            .continuity
            .recent_frames
            .last()
            .cloned()
            .expect("кадр показан и записан");
        let ui_backdrop = *self
            .backdrops
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .expect("UI-проход рисовал кадр");
        let held = matches!(record.label, FrameLabel::Transition(_)).then(|| self.held());
        Tick {
            record,
            ui_backdrop,
            holding: viewfinder.continuity.holding,
            held,
        }
    }

    pub fn command(&mut self, command: ViewfinderCommand) {
        assert_eq!(
            self.backend.apply_viewfinder_command(command),
            ViewfinderCommandOutcome::Applied
        );
    }

    pub fn counters(&mut self) -> render_loop::viewfinder::ContinuityCounters {
        self.backend
            .diagnostics()
            .viewfinder
            .expect("диагностика видоискателя")
            .continuity
            .counters
    }

    pub fn live(&self) -> ViewfinderReadback {
        self.backend
            .read_viewfinder_image()
            .expect("readback живого изображения")
            .expect("живое изображение есть")
    }

    pub fn held(&self) -> ViewfinderReadback {
        self.backend
            .read_held_image()
            .expect("readback удержанной картинки")
            .expect("удержанная картинка есть")
    }

    /// Показывает кадры потока (по кадру на тик), пока не придёт живой кадр; возвращает все тики.
    pub fn feed_until_live(&mut self, epoch: StreamEpoch, cell: CellValues) -> Vec<Tick> {
        let mut ticks = Vec::new();
        for _ in 0..MAX_TICKS {
            self.offer(epoch, cell);
            let tick = self.tick();
            let live = tick.record.label == FrameLabel::Live;
            ticks.push(tick);
            if live {
                return ticks;
            }
        }
        panic!(
            "живой кадр не пришёл за {MAX_TICKS} тиков: {:?}",
            ticks.last().map(|tick| tick.record.label)
        );
    }
}

/// Каждый пиксель картинки совпадает с `expected` в допуске; размер — `size`.
pub fn assert_picture(picture: &ViewfinderReadback, size: (u32, u32), expected: [f32; 3], what: &str) {
    assert_eq!((picture.width, picture.height), size, "{what}: размер содержимого");
    let worst = picture
        .pixels
        .iter()
        .map(|pixel| {
            (0..3)
                .map(|channel| (pixel[channel] - expected[channel]).abs())
                .fold(0.0_f32, f32::max)
        })
        .fold(0.0_f32, f32::max);
    assert!(
        worst <= TOLERANCE,
        "{what}: худшее отклонение {worst} > {TOLERANCE}; эталон {expected:?}, первый пиксель {:?}",
        picture.pixels.first()
    );
}

/// Размер содержимого выхода: половина кадрирования с поворотом сенсора 90° (стороны меняются местами).
pub fn content_size(buffer: PixelSize) -> (u32, u32) {
    (buffer.height / 2, buffer.width / 2)
}

pub fn finish(rig: &mut Rig, epochs: &[StreamEpoch]) {
    for epoch in epochs {
        rig.stream.producer().end_stream(*epoch);
    }
    assert!(rig.backend.detach().is_some(), "окно возвращено");
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_no_validation_errors(rig.backend.context());
}
