//! Стенд «белая в картинке» на GPU (RADV): синтетическая сцена ColorChecker идёт через боевой путь — импорт, ISP,
//! статистика, параметры ISP из указания владельца белого — и читается серая плитка и вся картинка.
//!
//! Общий для `vulkan_awb_scene.rs` (прежние сцены AWB, теперь с указанием) и `vulkan_white_*.rs` (источники,
//! холодный старт, смена потока). Роль владельца белого (движка) в тестах играет сам тест: публикует указания в
//! `WhiteLinks::directives` и читает ответы из `WhiteLinks::answers`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть помощников")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::awb_support::{FakeFrame, Returned, SceneImporter, fixture_tiles, mosaic_from_tiles, stream_passport};
use super::support::{Journal, fake_surface};
use super::vulkan_support::{
    assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_with_white_or_skip,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::frame_passport::{CaptureColorHints, ColorCorrectionGains, FrameCaptureDetails, StreamPassport};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::sync::GpuWaitFence;
use raw_frame_stream::{OfferedFrame, RawFrameStream, RawFrameStreamConfig, StreamEpoch};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentTarget};
use render_loop::config::{
    DisplayColorConfig, RenderLoopConfig, ToneBaseChoice, ViewfinderConfig, ViewfinderToneConfig, WhiteBalanceConfig,
};
use render_loop::viewfinder::WhiteLinks;
use render_loop::vulkan_backend::{ViewfinderReadback, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;

pub const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
/// Серая плитка сцены: белый образец ColorChecker (ряд 3, столбец 0 → плитки x 0–4, y 18–23), его центр.
pub const GRAY_PIXEL: (usize, usize) = (2, 20);
/// Допуск нейтральности серой плитки на выходе: 4 шага 10-битного выхода (квантование входа 14 бит и f32 шейдера).
pub const NEUTRAL_TOLERANCE: f32 = 4.0 / 1023.0;
pub const FIXTURE_SCENE: &str = "checker_bb2800";
pub const FIXTURE_LIGHT_KELVIN: f64 = 2800.0;

// ---- стенд ----

pub const FIRST_TS: i64 = 1_000_000_000;
pub const FRAME_NANOS: i64 = 16_666_667;
/// Кадры серии: метки далеко от обычных, чтобы импортёр различал их сцену.
pub const BRACKET_TS_BASE: i64 = 9_000_000_000;

pub fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

pub struct Rig {
    pub backend: VulkanDisplayBackend,
    pub stream: RawFrameStream,
    pub epoch: StreamEpoch,
    pub returned: Arc<Returned>,
    pub hints: CaptureColorHints,
    pub next_frame: u64,
    pub next_sequence: i64,
}

/// Результат одного кадра стенда: что показано и как выглядит серая плитка.
pub struct Shown {
    pub presented_before: u64,
    pub presented_after: u64,
    pub gray: Option<[f32; 3]>,
}

pub fn rig_or_skip(
    scenes: HashMap<i64, Arc<Vec<u16>>>,
    white: WhiteLinks,
    hints: CaptureColorHints,
    white_balance: WhiteBalanceConfig,
) -> Option<(Rig, Arc<Journal>)> {
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            white_balance,
            // Стенд проверяет баланс белого, а не тон: цветность плитки меряется на прежней S-кривой, у которой
            // нет сжатия плеча у белого (плечо AgX размывает цвет светлой плитки и занижает разброс каналов).
            // Основы по умолчанию (AgX и др.) проверяют тестами тона и паритета.
            tone: ViewfinderToneConfig {
                base: ToneBaseChoice::Legacy,
                ..ViewfinderToneConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    // Кадры идут по одному на показ: `LatestWins` выдаёт кадр сразу, без буфера с запасом.
    // Кольцо снимков здесь выключено осознанно: стенд про видоискатель и AWB, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, в том числе кадров серии, и камера получает их назад лишь по мере
    // вытеснения. Тест «кадры серии возвращены камере» без выключения падал бы из-за кольца, а не из-за
    // видоискателя. Поведение кольца со снимками проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let mut backend = headless_camera_backend_with_white_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(SceneImporter {
                context: context.clone(),
                scenes: scenes.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        white,
    )?;
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let epoch = stream.producer().begin_stream(Arc::new(stream_passport())).epoch;
    Some((
        Rig {
            backend,
            stream,
            epoch,
            returned: Arc::new(Returned::default()),
            hints,
            next_frame: 0,
            next_sequence: 1,
        },
        journal,
    ))
}

impl Rig {
    /// Предлагает кадр с меткой `sensor_ts` и происхождением `origin`.
    pub fn offer(&mut self, sensor_ts: i64, origin: RequestOrigin) {
        let producer = self.stream.producer();
        let now = Instant::now();
        producer.offer_frame(
            self.epoch,
            OfferedFrame {
                sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
                acquire_fence: GpuWaitFence::already_signaled(),
                frame: Box::new(FakeFrame {
                    sensor_ts,
                    storage: Box::new(0),
                    returned: Arc::clone(&self.returned),
                }),
            },
            now,
        );
        producer.offer_capture_result(
            self.epoch,
            FrameCaptureDetails {
                result: CaptureResultMeta {
                    frame_number: FrameNumber(self.next_sequence),
                    sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
                    exposure_time: None,
                    sensitivity_iso: Some(100),
                    reported_frame_duration: None,
                    rolling_shutter_skew: None,
                    three_a: ThreeAResultState::unreported(origin),
                },
                color: self.hints,
            },
            now,
        );
        self.next_sequence += 1;
    }

    pub fn presented(&mut self) -> u64 {
        self.backend
            .diagnostics()
            .camera
            .map_or(0, |camera| camera.camera_frames_presented)
    }

    /// Один кадр показа с новым кадром камеры; серая плитка читается, только если кадр показан.
    pub fn present_new_frame(&mut self, sensor_ts: i64, origin: RequestOrigin) -> Shown {
        self.offer(sensor_ts, origin);
        let presented_before = self.presented();
        assert_presented_or_skipped(self.backend.render_and_present(timing(self.next_frame)));
        self.next_frame += 1;
        let presented_after = self.presented();
        let gray = (presented_after > presented_before).then(|| self.gray_pixel());
        Shown {
            presented_before,
            presented_after,
            gray,
        }
    }

    /// Кадр показа без нового кадра камеры (перекомпоновка).
    pub fn tick(&mut self) {
        assert_presented_or_skipped(self.backend.render_and_present(timing(self.next_frame)));
        self.next_frame += 1;
    }

    pub fn readback(&self) -> ViewfinderReadback {
        self.backend
            .read_viewfinder_image()
            .expect("readback изображения видоискателя")
            .expect("изображение видоискателя есть")
    }

    pub fn gray_pixel(&self) -> [f32; 3] {
        let image = self.readback();
        assert_eq!(
            (image.width as usize, image.height as usize),
            (32, 24),
            "ячейка 2×2 → пиксель"
        );
        let pixel = image.pixels[GRAY_PIXEL.1 * image.width as usize + GRAY_PIXEL.0];
        [pixel[0], pixel[1], pixel[2]]
    }

    pub fn viewfinder(&mut self) -> render_loop::viewfinder::ViewfinderDiagnostics {
        self.backend.diagnostics().viewfinder.expect("диагностика видоискателя")
    }

    /// Новый поток камеры (смена линзы): паспорт нового потока вместо прежнего.
    pub fn begin_stream(&mut self, passport: StreamPassport) {
        self.stream.producer().end_stream(self.epoch);
        self.epoch = self.stream.producer().begin_stream(Arc::new(passport)).epoch;
    }

    /// Новая сессия показа: surface снимается и присоединяется заново (раннер белой создаётся заново).
    pub fn reattach_surface(&mut self, journal: &Arc<Journal>, surface_id: u64) {
        assert!(self.backend.detach().is_some(), "окно возвращено");
        self.backend
            .attach(fake_surface(journal, surface_id), SURFACE_SIZE)
            .expect("surface присоединяется");
    }

    /// Все пиксели изображения видоискателя сейчас.
    pub fn pixels(&self) -> Vec<[f32; 4]> {
        self.readback().pixels
    }

    pub fn finish(mut self, journal: &Arc<Journal>) {
        let _ = journal;
        self.stream.producer().end_stream(self.epoch);
        self.tick();
        assert!(self.backend.detach().is_some(), "окно возвращено");
        let counters = self.stream.counters();
        assert_eq!(counters.leaked_leases, 0, "{counters:?}");
        assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
        assert_no_validation_errors(self.backend.context());
    }
}

pub fn spread(pixel: [f32; 3]) -> f32 {
    let max = pixel.iter().copied().fold(f32::MIN, f32::max);
    let min = pixel.iter().copied().fold(f32::MAX, f32::min);
    max - min
}

pub fn scene_set(with_bracket: bool) -> HashMap<i64, Arc<Vec<u16>>> {
    let lamp = Arc::new(mosaic_from_tiles(&fixture_tiles(FIXTURE_SCENE)));
    let mut scenes: HashMap<i64, Arc<Vec<u16>>> = (0..60)
        .map(|index| (FIRST_TS + index * FRAME_NANOS, Arc::clone(&lamp)))
        .collect();
    if with_bracket {
        // Кадры серии: насыщенно-синяя сцена, которая испортила бы и картинку, и оценку света.
        let blue = Arc::new(mosaic_from_tiles(&vec![[0.05, 0.1, 0.6]; 768]));
        for index in 0..3 {
            scenes.insert(BRACKET_TS_BASE + index * FRAME_NANOS, Arc::clone(&blue));
        }
    }
    scenes
}

/// Подсказки HAL от другого света: коэффициенты дневного света (D65 ≈ 6500 К) при лампе 2800 К в кадре.
pub fn wild_hal_hints() -> CaptureColorHints {
    let rational = |numerator| camera_contract::capabilities::Rational {
        numerator,
        denominator: 1000,
    };
    CaptureColorHints {
        neutral_color_point: Some([rational(900), rational(1000), rational(200)]),
        color_gains: Some(ColorCorrectionGains([2.17, 1.0, 1.0, 1.46])),
        ..CaptureColorHints::default()
    }
}

/// Паспорт второй линзы: другая камера и ДРУГАЯ цветовая калибровка (поправка сенсора), поэтому
/// нейтраль одной и той же белой у неё своя.
pub fn second_lens_passport() -> StreamPassport {
    let mut passport = stream_passport();
    passport.camera = camera_contract::fixtures::camera_id("3");
    // Поправка экземпляра сенсора: красный канал чувствительнее на 10 %.
    for set in &mut passport.color_calibration {
        let red = &mut set.calibration_transform.0[0];
        red.numerator *= 11;
        red.denominator *= 10;
    }
    passport
}

/// Сцена-смена света: кадры `0..change_at` — лампа 2800 К, дальше — дневной ColorChecker (D65).
pub fn scene_with_light_change(frames: i64, change_at: i64) -> HashMap<i64, Arc<Vec<u16>>> {
    let lamp = Arc::new(mosaic_from_tiles(&fixture_tiles(FIXTURE_SCENE)));
    let day = Arc::new(mosaic_from_tiles(&fixture_tiles("checker_d65")));
    (0..frames)
        .map(|index| {
            let scene = if index < change_at { &lamp } else { &day };
            (FIRST_TS + index * FRAME_NANOS, Arc::clone(scene))
        })
        .collect()
}

/// Что стенд показал на кадре: пиксели изображения, если кадр показан.
pub type FrameImage = Option<Vec<[f32; 4]>>;

impl Rig {
    /// Гонит кадры `range`: подсказки по номеру кадра, после каждого кадра зовётся `after_frame` (роль движка).
    pub fn drive(
        &mut self,
        range: std::ops::Range<i64>,
        hints: &dyn Fn(i64) -> CaptureColorHints,
        after_frame: &mut dyn FnMut(i64),
    ) -> Vec<FrameImage> {
        let mut images = Vec::new();
        for index in range {
            self.hints = hints(index);
            let shown = self.present_new_frame(FIRST_TS + index * FRAME_NANOS, RequestOrigin::Repeating);
            images.push(shown.gray.is_some().then(|| self.pixels()));
            after_frame(index);
        }
        images
    }
}
