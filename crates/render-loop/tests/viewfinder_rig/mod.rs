//! Общий стенд «настоящий бэкенд показа + подставная камера»: однородные кадры RAW, поддельный производитель и
//! импортёр, эталон `isp-model` на тех же входах и сравнение изображения видоискателя с эталоном. Им пользуются
//! `vulkan_viewfinder_content.rs` (цвета выхода) и `vulkan_look_picture.rs` (вид картинки на пикселях).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тест-крейт берёт из стенда своё подмножество")]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use super::support::{Journal, fake_surface};
use super::vulkan_support::{
    assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_with_white_or_skip, held_white,
};
use camera_contract::auto_exposure::ExposureProposalChannel;
use camera_contract::capabilities::{LensShading, Rational, RawSupport, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::DirectiveSlot;
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
use isp_model::look::LookControls;
use isp_model::look::model::LookModelSettings;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference_encoded;
use isp_model::sensor::SensorDescription;
use look_contract::LookSettings;
use raw_frame_stream::{
    OfferedFrame, ProducerFrame, RawFrameLease, RawFrameStream, RawFrameStreamConfig, ReleaseFences, StreamEpoch,
};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentTarget};
use render_loop::config::{RenderLoopConfig, ViewfinderToneConfig};
use render_loop::viewfinder::AutoExposureLinks;
use render_loop::vulkan_backend::{
    CameraFrameImporter, CameraImportError, SubmissionSerial, ViewfinderReadback, VulkanDisplayBackend,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::SurfaceSize;

pub const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
pub const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
pub const BLACK_LEVEL: u16 = 64;
pub const WHITE_LEVEL: u16 = 1023;
/// Ручная точка белого, которую тест публикует через «почтовый ящик» указаний: 4000 К на кривой Планка.
/// Ручной режим на свежем потоке принимает точку сразу (без скрытия кадров и сглаживания), поэтому
/// эталон — чистая функция паспорта и этой точки.
pub const MANUAL_KELVIN: f64 = 4000.0;

/// Нейтраль камеры для ручной точки белого по DNG-профилю паспорта (то, что AWB отдаёт в ISP).
pub fn scene_neutral() -> [f64; 3] {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль цвета");
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(MANUAL_KELVIN),
        duv: Duv(0.0),
    })
    .expect("точка на кривой Планка");
    profile.neutral_for_white(white).expect("нейтраль").values()
}

/// Допуск: 10-битный выход (шаг 1/1023) плюс f32 шейдера против f32 эталона — 3 шага.
pub const TOLERANCE: f32 = 3.0 / 1023.0;
/// Сколько кадров показа ждать, пока новый кадр камеры дойдёт до изображения.
pub const MAX_FRAMES_TO_SETTLE: u64 = 20;

/// Значения RAW одной ячейки 2×2 по каналам (R, G, B) — одинаковые во всём кадре.
#[derive(Debug, Clone, Copy)]
pub struct CellValues {
    pub red: u16,
    pub green: u16,
    pub blue: u16,
}

/// Серый под нейтраль: сигнал над чёрным пропорционален нейтрали (40 % белого по зелёному).
pub fn neutral_gray() -> CellValues {
    let neutral = scene_neutral();
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL) * 0.4;
    let channel = |weight: f64| BLACK_LEVEL + (span * weight).round() as u16;
    CellValues {
        red: channel(neutral[0]),
        green: channel(neutral[1]),
        blue: channel(neutral[2]),
    }
}

/// Насыщенно-красный объект.
pub fn reddish() -> CellValues {
    CellValues {
        red: 600,
        green: 200,
        blue: 120,
    }
}

/// Отсчёт RAW в пикселе (x, y) раскладки BGGR.
pub fn bggr_sample(cell: CellValues, x: u32, y: u32) -> u16 {
    match (x % 2, y % 2) {
        (0, 0) => cell.blue,
        (1, 1) => cell.red,
        _ => cell.green,
    }
}

pub fn calibration() -> Vec<camera_contract::capabilities::ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

pub fn stream_passport() -> StreamPassport {
    StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: FRAME_SIZE,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: FRAME_SIZE,
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(FRAME_SIZE),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
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

/// Подсказки HAL в кадрах: НАРОЧНО неверные (нейтраль и коэффициенты баланса от другого света). ISP обязан их
/// игнорировать целиком (решение 62, критерий выхода 08): эталон считается по нейтрали применённой белой.
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

/// Эталон `isp-model` на тех же входах, что видит бэкенд: цвет пикселя выхода для ячейки `cell`.
pub fn reference_color(passport: &StreamPassport, cell: CellValues) -> [f32; 3] {
    reference_color_with_tone(passport, cell, &ViewfinderToneConfig::default())
}

/// Эталон `isp-model` ячейки `cell` с настройками тона `tone` (подтяжки AE нет: стенд без нашего AE).
pub fn reference_color_with_tone(passport: &StreamPassport, cell: CellValues, tone: &ViewfinderToneConfig) -> [f32; 3] {
    reference_color_with_look(passport, cell, tone, &LookSettings::NEUTRAL)
}

/// Эталон `isp-model` ячейки `cell` с тоном `tone` и видом `look` (модель вида — умолчание, как у стенда).
pub fn reference_color_with_look(
    passport: &StreamPassport,
    cell: CellValues,
    tone: &ViewfinderToneConfig,
    look: &LookSettings,
) -> [f32; 3] {
    let sensor = SensorDescription::from_stream_passport(passport, &passport.color_calibration).expect("сенсор");
    let hints = capture_hints();
    let frame = FrameInputs::from_capture_hints(&sensor, &hints).expect("уровни");
    let neutral = CameraNeutral::new(scene_neutral()).expect("нейтраль");
    let controls = ProcessingControls {
        tone: tone.curve_settings(),
        tone_base: tone.tone_base(),
        look: LookControls::new(look.clone(), Arc::new(LookModelSettings::default())),
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Preview, DisplayGamut::Srgb)
    };
    let params = IspParams::resolve(&sensor, &frame, &controls).expect("параметры");
    let size = PixelSize::new(4, 4);
    let samples = (0..16).map(|index| bggr_sample(cell, index % 4, index / 4)).collect();
    let raw = RawMosaic::new(size, BayerPattern::BGGR, samples).expect("мозаика");
    run_reference_encoded(&raw, &params).expect("эталон").pixel(0, 0)
}

// ---- поддельный производитель и импортёр ----

#[derive(Default)]
pub struct Returned(pub Mutex<Vec<i64>>);

pub struct FakeFrame {
    sensor_ts: i64,
    storage: Box<u8>,
    returned: Arc<Returned>,
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

/// Импортёр: каждый кадр — однородная ячейка `CellValues` по метке сенсора.
pub struct UniformImporter {
    pub context: GpuContext,
    pub cells: HashMap<i64, CellValues>,
    pub entries: HashMap<i64, (SyntheticRaw16, u64)>,
    pub counters: ImportCacheCounters,
}

impl CameraFrameImporter for UniformImporter {
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
            .cells
            .get(&sensor_ts)
            .ok_or_else(|| CameraImportError::Platform(format!("неизвестный кадр {sensor_ts}")))?;
        let texels: Vec<u16> = (0..FRAME_SIZE.height)
            .flat_map(|y| (0..FRAME_SIZE.width).map(move |x| bggr_sample(cell, x, y)))
            .collect();
        let source = SyntheticRaw16::new(&self.context, FRAME_SIZE.width, FRAME_SIZE.height, &texels)?;
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

// ---- стенд ----

pub const GRAY_TS: i64 = 1_000_000_000;
pub const RED_TS: i64 = 1_016_666_667;

pub fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

/// Предлагает кадр с меткой `sensor_ts` и его метаданными.
pub fn offer(stream: &RawFrameStream, epoch: StreamEpoch, sensor_ts: i64, sequence: i64, returned: &Arc<Returned>) {
    let producer = stream.producer();
    let now = Instant::now();
    producer.offer_frame(
        epoch,
        OfferedFrame {
            sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
            acquire_fence: GpuWaitFence::already_signaled(),
            frame: Box::new(FakeFrame {
                sensor_ts,
                storage: Box::new(0),
                returned: Arc::clone(returned),
            }),
        },
        now,
    );
    producer.offer_capture_result(
        epoch,
        FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(sequence),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
                // Выдержка с ISO — то, по чему наш AE (если он подключён) измеряет кадр; без AE ни на что не влияет.
                exposure_time: Some(std::time::Duration::from_millis(10)),
                sensitivity_iso: Some(100),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: camera_contract::three_a::ThreeAResultState::unreported(
                    camera_contract::three_a::RequestOrigin::Repeating,
                ),
            },
            color: capture_hints(),
        },
        now,
    );
}

/// Рисует кадры, пока ISP не обработает `processed_target` кадров; возвращает следующий номер кадра.
pub fn present_until_processed(backend: &mut VulkanDisplayBackend, mut frame_index: u64, processed_target: u64) -> u64 {
    for _ in 0..MAX_FRAMES_TO_SETTLE {
        assert_presented_or_skipped(backend.render_and_present(timing(frame_index)));
        frame_index += 1;
        let processed = backend
            .diagnostics()
            .viewfinder
            .map_or(0, |viewfinder| viewfinder.counters.frames_processed);
        if processed >= processed_target {
            // Ещё кадр: перекомпоновка (изображение не меняется, экран собирается из него).
            assert_presented_or_skipped(backend.render_and_present(timing(frame_index)));
            return frame_index + 1;
        }
    }
    panic!(
        "ISP не обработал {processed_target} кадров за {MAX_FRAMES_TO_SETTLE} кадров показа: {:?}",
        backend.diagnostics().viewfinder
    );
}

/// Каждый пиксель изображения видоискателя совпадает с `expected` в пределах допуска.
pub fn assert_image_color(backend: &VulkanDisplayBackend, expected: [f32; 3], what: &str) {
    let ViewfinderReadback { width, height, pixels } = backend
        .read_viewfinder_image()
        .expect("readback изображения видоискателя")
        .expect("изображение видоискателя есть");
    // Кадр 64×48, поворот сенсора 90°: прямое изображение 24×32 (ячейка 2×2 → пиксель).
    assert_eq!(
        (width, height),
        (FRAME_SIZE.height / 2, FRAME_SIZE.width / 2),
        "{what}: размер"
    );
    let worst = pixels
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
        pixels.first()
    );
}

// ---- стенд «один сюжет, много кадров» ----

/// Шаг меток сенсора между кадрами стенда (60 к/с).
const FRAME_STEP_NANOS: i64 = 16_666_667;

/// Подключён ли к стенду наш AE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoExposureWiring {
    /// Вендорский AE: на потоке рендера потребителей статистики нет.
    Off,
    /// Наш AE на потоке рендера.
    On,
}

/// Стенд видоискателя: настоящий бэкенд, поток кадров и `frames` одинаковых кадров сюжета `scene`.
/// Каждый вызов `show_next_frame` предлагает следующий кадр и доводит его до изображения видоискателя.
pub struct ViewfinderBench {
    pub backend: VulkanDisplayBackend,
    pub stream: RawFrameStream,
    pub passport: Arc<StreamPassport>,
    pub epoch: StreamEpoch,
    pub returned: Arc<Returned>,
    frame_index: u64,
    shown_frames: u64,
}

impl ViewfinderBench {
    /// Бэкенд с ручной точкой белого (эталон — чистая функция паспорта). `None` — GPU осознанно пропущен.
    pub fn start(render_config: &RenderLoopConfig, scene: CellValues, frames: usize) -> Option<Self> {
        Self::start_with_scenes(render_config, &vec![scene; frames])
    }

    /// То же, но кадр номер `i` показывает сюжет `scenes[i]`: сквозные тесты приборов отличают «прибор измерил
    /// именно этот кадр» от «прибор измерил какой-то из кадров».
    pub fn start_with_scenes(render_config: &RenderLoopConfig, scenes: &[CellValues]) -> Option<Self> {
        Self::start_wired(render_config, scenes, AutoExposureWiring::Off)
    }

    /// То же, что `start`, но с нашим AE на потоке рендера: единственный потребитель статистики кадра (без него
    /// статистика не считается). Кадры стенда несут выдержку и ISO, поэтому AE их измеряет.
    pub fn start_with_auto_exposure(
        render_config: &RenderLoopConfig,
        scene: CellValues,
        frames: usize,
    ) -> Option<Self> {
        Self::start_with_scenes_and_auto_exposure(render_config, &vec![scene; frames])
    }

    /// `start_with_scenes` с нашим AE на потоке рендера (см. `start_with_auto_exposure`).
    pub fn start_with_scenes_and_auto_exposure(
        render_config: &RenderLoopConfig,
        scenes: &[CellValues],
    ) -> Option<Self> {
        Self::start_wired(render_config, scenes, AutoExposureWiring::On)
    }

    fn start_wired(
        render_config: &RenderLoopConfig,
        scenes: &[CellValues],
        auto_exposure: AutoExposureWiring,
    ) -> Option<Self> {
        // `LatestWins`: одиночный кадр показывается сразу; кольцо снимков выключено (тесты про видоискатель).
        let stream = RawFrameStream::new(RawFrameStreamConfig {
            viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
            snapshot_ring_max_frames: 0,
            ..RawFrameStreamConfig::default()
        });
        let cells: HashMap<i64, CellValues> = scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| (frame_timestamp(index), *scene))
            .collect();
        let white = held_white(MANUAL_KELVIN);
        let backend = headless_camera_backend_with_white_or_skip(
            render_config,
            stream.viewfinder(),
            Box::new(move |context| {
                Ok(Box::new(UniformImporter {
                    context: context.clone(),
                    cells: cells.clone(),
                    entries: HashMap::new(),
                    counters: ImportCacheCounters::default(),
                }))
            }),
            white,
        )?;
        let mut backend = match auto_exposure {
            AutoExposureWiring::Off => backend,
            AutoExposureWiring::On => backend.with_auto_exposure(AutoExposureLinks {
                directives: DirectiveSlot::new(),
                proposals: ExposureProposalChannel::new(),
            }),
        };
        let journal = Arc::new(Journal::default());
        backend
            .attach(fake_surface(&journal, 1), SURFACE_SIZE)
            .expect("surface присоединяется");
        let passport = Arc::new(stream_passport());
        let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
        Some(Self {
            backend,
            stream,
            passport,
            epoch,
            returned: Arc::new(Returned::default()),
            frame_index: 0,
            shown_frames: 0,
        })
    }

    /// Предлагает следующий кадр сюжета и ждёт, пока он дойдёт до изображения видоискателя.
    pub fn show_next_frame(&mut self) {
        let index = usize::try_from(self.shown_frames).expect("номер кадра");
        let sequence = i64::try_from(self.shown_frames).expect("номер кадра") + 1;
        offer(
            &self.stream,
            self.epoch,
            frame_timestamp(index),
            sequence,
            &self.returned,
        );
        self.shown_frames += 1;
        self.frame_index = present_until_processed(&mut self.backend, self.frame_index, self.shown_frames);
    }

    /// Рисует ровно один кадр показа без нового кадра камеры (перекомпоновка): тесты, которым важно состояние
    /// между соседними кадрами (выведенные объекты ждут освобождения слотов).
    pub fn present_one_recompose_frame(&mut self) {
        assert_presented_or_skipped(self.backend.render_and_present(timing(self.frame_index)));
        self.frame_index += 1;
    }

    /// Останавливает поток, отсоединяет окно и проверяет, что утечек лизингов и ошибок validation нет.
    pub fn finish(mut self) {
        self.stream.producer().end_stream(self.epoch);
        assert!(self.backend.detach().is_some(), "окно возвращено");
        let counters = self.stream.counters();
        assert_eq!(counters.leaked_leases, 0, "{counters:?}");
        assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
        assert_no_validation_errors(self.backend.context());
    }
}

/// Метка сенсора кадра номер `index` стенда.
fn frame_timestamp(index: usize) -> i64 {
    GRAY_TS + FRAME_STEP_NANOS * i64::try_from(index).expect("номер кадра")
}
