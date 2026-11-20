//! Цвета выхода ISP через НАСТОЯЩИЙ бэкенд показа: `VulkanDisplayBackend` в режиме камеры на реальном
//! драйвере (RADV, headless-показ, полная validation) + настоящий `RawFrameStream`. Синтетический RAW
//! (каждая ячейка 2×2 одинакова) проходит боевой путь — опрос, параметры ISP из паспорта и
//! подсказок кадра, импорт, ISP, компоновка, возврат лизинга, — и изображение видоискателя читается
//! обратно (тестовый шов `read_viewfinder_image`).
//!
//! Доказывается: цвет каждого пикселя изображения совпадает с эталоном `isp-model` на ТЕХ ЖЕ входах
//! (паспорт, нейтраль HAL, гамма экрана); «серый под нейтраль» выходит серым; цветной кадр сменяет
//! серый (изображение действительно обновляется новым кадром); после остановки потока и отсоединения
//! все лизинги возвращены, объекты Vulkan вернулись к исходным, validation чиста.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod support;
mod vulkan_support;

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

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
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ViewfinderConfig, ViewfinderToneConfig};
use render_loop::vulkan_backend::{
    CameraFrameImporter, CameraImportError, SubmissionSerial, ViewfinderReadback, VulkanDisplayBackend,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::SurfaceSize;
use support::{Journal, fake_surface};
use vulkan_support::{assert_no_validation_errors, headless_camera_backend_with_directives_or_skip};

const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
const BLACK_LEVEL: u16 = 64;
const WHITE_LEVEL: u16 = 1023;
/// Ручная точка белого, которую тест публикует через «почтовый ящик» указаний: 4000 К на кривой Планка.
/// Ручной режим на свежем потоке принимает точку сразу (без скрытия кадров и сглаживания), поэтому
/// эталон — чистая функция паспорта и этой точки.
const MANUAL_KELVIN: f64 = 4000.0;

/// Нейтраль камеры для ручной точки белого по DNG-профилю паспорта (то, что AWB отдаёт в ISP).
fn scene_neutral() -> [f64; 3] {
    let profile = DngColorProfile::from_calibration_sets(&calibration()).expect("профиль цвета");
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(MANUAL_KELVIN),
        duv: Duv(0.0),
    })
    .expect("точка на кривой Планка");
    profile.neutral_for_white(white).expect("нейтраль").values()
}

fn manual_white_directive() -> WhiteBalanceDirective {
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
/// Допуск: 10-битный выход (шаг 1/1023) плюс f32 шейдера против f32 эталона — 3 шага.
const TOLERANCE: f32 = 3.0 / 1023.0;
/// Сколько кадров показа ждать, пока новый кадр камеры дойдёт до изображения.
const MAX_FRAMES_TO_SETTLE: u64 = 20;

/// Значения RAW одной ячейки 2×2 по каналам (R, G, B) — одинаковые во всём кадре.
#[derive(Debug, Clone, Copy)]
struct CellValues {
    red: u16,
    green: u16,
    blue: u16,
}

/// Серый под нейтраль: сигнал над чёрным пропорционален нейтрали (40 % белого по зелёному).
fn neutral_gray() -> CellValues {
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
fn reddish() -> CellValues {
    CellValues {
        red: 600,
        green: 200,
        blue: 120,
    }
}

/// Отсчёт RAW в пикселе (x, y) раскладки BGGR.
fn bggr_sample(cell: CellValues, x: u32, y: u32) -> u16 {
    match (x % 2, y % 2) {
        (0, 0) => cell.blue,
        (1, 1) => cell.red,
        _ => cell.green,
    }
}

fn calibration() -> Vec<camera_contract::capabilities::ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

fn stream_passport() -> StreamPassport {
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
/// игнорировать целиком (решение 62, критерий выхода 08): эталон считается по нейтрали нашего AWB.
fn capture_hints() -> CaptureColorHints {
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
fn reference_color(passport: &StreamPassport, cell: CellValues) -> [f32; 3] {
    reference_color_with_tone(passport, cell, ToneCurveSettings::default())
}

/// Эталон `isp-model` ячейки `cell` с настройками тона `tone` (подтяжки AE нет: стенд без нашего AE).
fn reference_color_with_tone(passport: &StreamPassport, cell: CellValues, tone: ToneCurveSettings) -> [f32; 3] {
    let sensor = SensorDescription::from_stream_passport(passport, &passport.color_calibration).expect("сенсор");
    let hints = capture_hints();
    let frame = FrameInputs::from_capture_hints(&sensor, &hints).expect("уровни");
    let neutral = CameraNeutral::new(scene_neutral()).expect("нейтраль");
    let controls = ProcessingControls {
        tone,
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Preview, DisplayGamut::Srgb)
    };
    let params = IspParams::resolve(&sensor, &frame, &controls).expect("параметры");
    let size = PixelSize::new(4, 4);
    let samples = (0..16).map(|index| bggr_sample(cell, index % 4, index / 4)).collect();
    let raw = RawMosaic::new(size, BayerPattern::BGGR, samples).expect("мозаика");
    run_reference(&raw, &params)
        .expect("эталон")
        .display_encoded
        .pixel(0, 0)
}

// ---- поддельный производитель и импортёр ----

#[derive(Default)]
struct Returned(Mutex<Vec<i64>>);

struct FakeFrame {
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
struct UniformImporter {
    context: GpuContext,
    cells: HashMap<i64, CellValues>,
    entries: HashMap<i64, (SyntheticRaw16, u64)>,
    counters: ImportCacheCounters,
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

const GRAY_TS: i64 = 1_000_000_000;
const RED_TS: i64 = 1_016_666_667;

fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

/// Предлагает кадр с меткой `sensor_ts` и его метаданными.
fn offer(stream: &RawFrameStream, epoch: StreamEpoch, sensor_ts: i64, sequence: i64, returned: &Arc<Returned>) {
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
                exposure_time: None,
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
fn present_until_processed(backend: &mut VulkanDisplayBackend, mut frame_index: u64, processed_target: u64) -> u64 {
    for _ in 0..MAX_FRAMES_TO_SETTLE {
        assert_eq!(
            backend.render_and_present(timing(frame_index)),
            PresentOutcome::Presented
        );
        frame_index += 1;
        let processed = backend
            .diagnostics()
            .viewfinder
            .map_or(0, |viewfinder| viewfinder.counters.frames_processed);
        if processed >= processed_target {
            // Ещё кадр: перекомпоновка (изображение не меняется, экран собирается из него).
            assert_eq!(
                backend.render_and_present(timing(frame_index)),
                PresentOutcome::Presented
            );
            return frame_index + 1;
        }
    }
    panic!(
        "ISP не обработал {processed_target} кадров за {MAX_FRAMES_TO_SETTLE} кадров показа: {:?}",
        backend.diagnostics().viewfinder
    );
}

/// Каждый пиксель изображения видоискателя совпадает с `expected` в пределах допуска.
fn assert_image_color(backend: &VulkanDisplayBackend, expected: [f32; 3], what: &str) {
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

#[test]
fn isp_output_through_the_real_backend_matches_the_isp_model_reference() {
    let render_config = RenderLoopConfig {
        // Headless-стенд и так даёт sRGB; явный выбор делает эталон однозначным.
        display_color: DisplayColorConfig::SrgbOnly,
        ..RenderLoopConfig::default()
    };
    // Тест проверяет путь кадра «первый же vsync», а не политику видоискателя: `LatestWins`
    // выдаёт одиночный кадр сразу (буфер с запасом ждал бы второй — см. raw-frame-stream и стенд).
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        ..RawFrameStreamConfig::default()
    });
    let cells = HashMap::from([(GRAY_TS, neutral_gray()), (RED_TS, reddish())]);
    let directives = DirectiveSlot::new();
    directives.publish(manual_white_directive());
    let Some(mut backend) = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(UniformImporter {
                context: context.clone(),
                cells: cells.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        directives,
    ) else {
        return;
    };
    let baseline = backend.context().live_objects();
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let passport = Arc::new(stream_passport());
    let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
    let returned = Arc::new(Returned::default());

    // 1. Серый под нейтраль.
    offer(&stream, epoch, GRAY_TS, 1, &returned);
    let mut frame_index = present_until_processed(&mut backend, 0, 1);
    let gray = reference_color(&passport, neutral_gray());
    let spread = gray.iter().copied().fold(f32::MIN, f32::max) - gray.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        spread < 0.01 && gray[1] > 0.2,
        "эталон серого — серый средней яркости: {gray:?}"
    );
    assert_image_color(&backend, gray, "серый");

    // 2. Красный кадр сменяет серый: изображение обновилось новым кадром.
    offer(&stream, epoch, RED_TS, 2, &returned);
    frame_index = present_until_processed(&mut backend, frame_index, 2);
    let red = reference_color(&passport, reddish());
    assert!(
        red[0] > red[1] + 0.2 && red[0] > red[2] + 0.2,
        "эталон красного — красный: {red:?}"
    );
    assert_image_color(&backend, red, "красный");

    // Оба кадра возвращены производителю во время показа (каждый — своим сабмитом).
    assert_eq!(
        *returned.0.lock().unwrap_or_else(PoisonError::into_inner),
        vec![GRAY_TS, RED_TS],
        "кадры ISP возвращаются сразу, а не ждут следующего кадра"
    );

    // 3. Остановка потока: чёрный кадр, затем отсоединение без утечек.
    stream.producer().end_stream(epoch);
    assert_eq!(
        backend.render_and_present(timing(frame_index)),
        PresentOutcome::Presented
    );
    let viewfinder = backend.diagnostics().viewfinder.expect("диагностика видоискателя");
    assert_eq!(
        viewfinder.counters.params_resolved, 1,
        "входы не менялись: параметры посчитаны один раз"
    );
    assert!(viewfinder.counters.recomposed >= 2, "{viewfinder:?}");
    assert_eq!(viewfinder.counters.isp_record_failures, 0, "{viewfinder:?}");
    assert!(backend.detach().is_some(), "окно возвращено");
    let counters = stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(
        backend.context().live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    assert_no_validation_errors(backend.context());
}

/// Ручки тона из конфига (`viewfinder.tone`, сессия 12b) доходят до показанного кадра: серый с контрастом 1,8 и
/// серединой экрана 0,24 совпадает с эталоном `isp-model` с ТЕМИ ЖЕ ручками и заметно отличается от серого по
/// умолчанию. Если конфиг до кэша параметров не дойдёт, ISP нарисует серый по умолчанию и тест упадёт.
#[test]
fn tone_knobs_from_the_config_reach_the_shown_viewfinder_image() {
    let tone = ViewfinderToneConfig {
        contrast: 1.8,
        display_middle_gray: 0.24,
    };
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            tone,
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    render_config
        .validate()
        .expect("конфиг с ручками тона проходит проверку");
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        ..RawFrameStreamConfig::default()
    });
    let cells = HashMap::from([(GRAY_TS, neutral_gray())]);
    let directives = DirectiveSlot::new();
    directives.publish(manual_white_directive());
    let Some(mut backend) = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(UniformImporter {
                context: context.clone(),
                cells: cells.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        directives,
    ) else {
        return;
    };
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let passport = Arc::new(stream_passport());
    let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
    let returned = Arc::new(Returned::default());

    offer(&stream, epoch, GRAY_TS, 1, &returned);
    present_until_processed(&mut backend, 0, 1);

    let configured = reference_color_with_tone(&passport, neutral_gray(), tone.curve_settings());
    let default = reference_color(&passport, neutral_gray());
    // Эталоны должны различаться сильнее допуска сравнения, иначе тест не отличил бы «дошло» от «не дошло».
    let separation = (configured[1] - default[1]).abs();
    assert!(
        separation > 4.0 * TOLERANCE,
        "ручки тона почти не меняют серый ({configured:?} против {default:?}): тест ничего не докажет"
    );
    assert_image_color(&backend, configured, "серый с ручками тона из конфига");

    stream.producer().end_stream(epoch);
    assert!(backend.detach().is_some(), "окно возвращено");
    assert_eq!(stream.counters().leaked_leases, 0);
    assert_no_validation_errors(backend.context());
}
