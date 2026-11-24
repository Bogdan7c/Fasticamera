//! Стенд AE на настоящем драйвере: камера + движок + сцена с яркостью, зависящей от экспозиции кадра
//! (используется `vulkan_ae_scene.rs`).

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::awb_support::{FRAME_SIZE, FakeFrame, Returned, fixture_tiles, mosaic_from_tiles, stream_passport};
use super::support::{Journal, fake_surface};
use super::vulkan_support::{
    assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_with_directives_or_skip,
};
use camera_contract::auto_exposure::{
    AppliedExposure, ExposureAuthority, ExposureDirective, ExposureProduct, ExposureProposalChannel,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::control::{ColorTemperature, ExposureBias, Tint, WhiteBalanceSetting};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails};
use camera_contract::three_a::{
    AeAfLock, DirectiveGeneration, DirectiveSlot, PointOfInterest, RequestOrigin, ThreeAProfile, ThreeAResultState,
    WhiteBalanceDirective, WhiteBalanceMode,
};
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::sync::GpuWaitFence;
use isp_model::tone::{ToneCurve, ToneCurveSettings};
use isp_model::transfer::srgb_decode;
use raw_frame_stream::{OfferedFrame, RawFrameLease, RawFrameStream, RawFrameStreamConfig, StreamEpoch};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentTarget};
use render_loop::config::{AutoExposureConfig, DisplayColorConfig, RenderLoopConfig, ViewfinderConfig};
use render_loop::viewfinder::{AutoExposureLinks, ViewfinderDiagnostics};
use render_loop::vulkan_backend::{
    CameraFrameImporter, CameraImportError, SubmissionSerial, ViewfinderReadback, VulkanDisplayBackend,
};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;

pub const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
pub const FIRST_TS: i64 = 1_000_000_000;
pub const FRAME_NANOS: i64 = 16_666_667;
pub const BRACKET_TS_BASE: i64 = 9_000_000_000;
/// Через сколько кадров новая экспозиция запроса видна на кадрах (задержка конвейера сенсора: 7–8 кадров).
pub const SENSOR_DELAY_FRAMES: i64 = 8;
/// Белая плитка ColorChecker в кадре лампы 2800 К: её центр в фикстуре.
pub const WHITE_TILE: usize = 20 * 32 + 2;
/// ISO кадров стенда; меняется только выдержка.
pub const ISO: u32 = 100;
/// Плиток статистики в кадре стенда (32×24: одна ячейка 2×2 на плитку).
pub const TILE_COUNT: usize = 32 * 24;
/// Масштаб сцены: подобран так, что цель замера ≈ 1 с·ISO (проверяется тестом `the_scene_is_calibrated...`).
pub const SCENE_SCALE: f64 = 0.0748;
/// Старт на 2 EV темнее цели.
pub const DARK_START_PRODUCT: f64 = 0.25;

pub fn timing(frame_index: u64) -> FrameTiming {
    let expected_present = MonotonicNanos::from_nanos(1_000_000 * i64::try_from(frame_index).expect("кадр"));
    FrameTiming {
        frame_index,
        vsync_sequence: frame_index + 1,
        skipped_vsyncs: 0,
        target: PresentTarget::latch_immediately(expected_present, expected_present),
    }
}

// ---- импортёр, мозаики которого тест создаёт по ходу (яркость зависит от экспозиции кадра) ----

pub type SharedScenes = Arc<Mutex<HashMap<i64, Arc<Vec<u16>>>>>;

pub struct LiveSceneImporter {
    context: gpu_vulkan::context::GpuContext,
    scenes: SharedScenes,
    entries: HashMap<i64, (SyntheticRaw16, u64)>,
    counters: ImportCacheCounters,
}

impl CameraFrameImporter for LiveSceneImporter {
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
        let texels = self
            .scenes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&sensor_ts)
            .cloned()
            .ok_or_else(|| CameraImportError::Platform(format!("неизвестный кадр {sensor_ts}")))?;
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

// ---- стенд: камера + движок + сцена ----

/// Что видно на экране после кадра.
#[derive(Debug, Clone, Copy)]
pub struct Shot {
    pub frame: i64,
    /// «Выдержка × ISO», стоявшая на кадре.
    pub sensor_product: f64,
    /// Яркость плашки на экране, линейный свет (`None` — кадр не показан).
    pub display_luminance: Option<f64>,
    /// Закодированный пиксель плашки (`None` — кадр не показан).
    pub pixel: Option<[f32; 3]>,
}

pub struct AeRig {
    pub backend: VulkanDisplayBackend,
    pub stream: RawFrameStream,
    pub epoch: StreamEpoch,
    pub returned: Arc<Returned>,
    pub scenes: SharedScenes,
    pub directives: DirectiveSlot<ExposureDirective>,
    pub proposals: ExposureProposalChannel,
    /// Цвет белой плитки лампы, нормированный по наибольшему каналу.
    pub tile: [f64; 3],
    /// Масштаб сцены и разрешение пересвета (см. `RigSetup`).
    pub scene_scale: f64,
    pub allow_clipping: bool,
    /// Распределение яркости по плиткам.
    pub layout: SceneLayout,
    /// Освещённость сцены относительно исходной (1 — исходная).
    pub light: f64,
    /// Что реально стоит на кадрах сейчас.
    pub sensor_product: f64,
    /// Что стоит в запросе камеры (видно на кадрах через задержку).
    pub commanded_product: f64,
    pub pending: VecDeque<(i64, f64)>,
    pub seen: Option<DirectiveGeneration>,
    pub lock: AeAfLock,
    pub frame: i64,
    pub render_index: u64,
    pub sequence: i64,
    pub history: Vec<Shot>,
    /// Кадр, на котором камера впервые увидела предложение (и опубликовала `applied`).
    pub first_applied_at: Option<i64>,
}

/// Белая точка лампы 2800 К задана вручную: AWB не прячет тёмные первые кадры и не гуляет вместе с экспозицией,
/// поэтому в тесте меняется только экспозиция.
pub fn manual_lamp_white() -> DirectiveSlot<WhiteBalanceDirective> {
    let directives = DirectiveSlot::new();
    directives.publish(WhiteBalanceDirective {
        mode: WhiteBalanceMode::Manual {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(2800.0).expect("температура"),
                tint: Tint::NEUTRAL,
            },
        },
        profile: ThreeAProfile::Photo,
    });
    directives
}

/// Чем отличаются сценарии стенда.
pub struct RigSetup {
    /// Настройки AE (`None` — ящики AE не переданы).
    pub auto_exposure: Option<AutoExposureConfig>,
    /// Указания AWB (`None` — AWB автоматический, как в продукте).
    pub white_balance: Option<DirectiveSlot<WhiteBalanceDirective>>,
    /// Сцена фикстуры, чья белая плитка заливает кадр.
    pub scene: &'static str,
    /// Экспозиция первых кадров, с·ISO.
    pub start_product: f64,
    /// Масштаб сцены (яркость кадра = масштаб × свет × экспозиция).
    pub scene_scale: f64,
    /// Разрешено ли пересвечивать кадры (значения обрезаются белым сенсора).
    pub allow_clipping: bool,
    /// Как яркость распределена по плиткам кадра.
    pub layout: SceneLayout,
}

/// Распределение яркости сцены по плиткам статистики (сетка 32×24 = кадр стенда).
#[derive(Debug, Clone, PartialEq)]
pub enum SceneLayout {
    /// Вся сцена одной яркости (серая плашка на весь кадр).
    Uniform,
    /// Своя относительная отражательная способность у каждой плитки (строки сверху вниз, 768 чисел).
    PerTile(Vec<f64>),
}

impl SceneLayout {
    /// Относительная яркость плитки `index` (1 — как у ровной сцены).
    pub fn reflectance(&self, index: usize) -> f64 {
        match self {
            Self::Uniform => 1.0,
            Self::PerTile(tiles) => tiles[index],
        }
    }
}

pub fn rig_or_skip(auto_exposure: Option<AutoExposureConfig>) -> Option<(AeRig, Arc<Journal>)> {
    rig_with(RigSetup {
        auto_exposure,
        white_balance: Some(manual_lamp_white()),
        scene: "checker_bb2800",
        start_product: DARK_START_PRODUCT,
        scene_scale: SCENE_SCALE,
        allow_clipping: false,
        layout: SceneLayout::Uniform,
    })
}

pub fn rig_with(setup: RigSetup) -> Option<(AeRig, Arc<Journal>)> {
    let RigSetup {
        auto_exposure,
        white_balance,
        scene,
        start_product,
        scene_scale,
        allow_clipping,
        layout,
    } = setup;
    let enabled = auto_exposure.is_some();
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        viewfinder: ViewfinderConfig {
            auto_exposure: auto_exposure.unwrap_or_default(),
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        ..RawFrameStreamConfig::default()
    });
    let scenes: SharedScenes = Arc::default();
    let importer_scenes = Arc::clone(&scenes);
    let backend = headless_camera_backend_with_directives_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(move |context| {
            Ok(Box::new(LiveSceneImporter {
                context: context.clone(),
                scenes: Arc::clone(&importer_scenes),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
        white_balance.unwrap_or_default(),
    )?;
    let directives = DirectiveSlot::new();
    let proposals = ExposureProposalChannel::new();
    let mut backend = if enabled {
        backend.with_auto_exposure(AutoExposureLinks {
            directives: directives.clone(),
            proposals: proposals.clone(),
        })
    } else {
        backend
    };
    let journal = Arc::new(Journal::default());
    backend
        .attach(fake_surface(&journal, 1), SURFACE_SIZE)
        .expect("surface присоединяется");
    let epoch = stream.producer().begin_stream(Arc::new(stream_passport())).epoch;
    let tile = fixture_tiles(scene)[WHITE_TILE];
    let brightest = tile.into_iter().fold(f64::MIN, f64::max);
    Some((
        AeRig {
            backend,
            stream,
            epoch,
            returned: Arc::new(Returned::default()),
            scenes,
            directives,
            proposals,
            tile: tile.map(|channel| channel / brightest),
            light: 1.0,
            sensor_product: start_product,
            commanded_product: start_product,
            scene_scale,
            allow_clipping,
            layout,
            pending: VecDeque::new(),
            seen: None,
            lock: AeAfLock::Unlocked,
            frame: 0,
            render_index: 0,
            sequence: 1,
            history: Vec::new(),
            first_applied_at: None,
        },
        journal,
    ))
}

impl AeRig {
    pub fn timestamp(&self) -> i64 {
        FIRST_TS + self.frame * FRAME_NANOS
    }

    /// Мозаика сцены: яркость пропорциональна экспозиции кадра и отражению плитки, цвет — белая плитка лампы.
    pub fn insert_scene(&self, sensor_ts: i64, sensor_product: f64) {
        let level = self.scene_scale * self.light * sensor_product;
        let tiles: Vec<[f64; 3]> = (0..TILE_COUNT)
            .map(|index| {
                let rgb = self
                    .tile
                    .map(|channel| channel * level * self.layout.reflectance(index));
                if self.allow_clipping {
                    // Белый сенсора: пересвеченный канал не бывает ярче 1.
                    rgb.map(|channel| channel.min(1.0))
                } else {
                    assert!(
                        rgb.iter().all(|channel| *channel < 0.9),
                        "сцена не должна пересвечиваться в тесте: плитка {index} {rgb:?}"
                    );
                    rgb
                }
            })
            .collect();
        self.scenes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(sensor_ts, Arc::new(mosaic_from_tiles(&tiles)));
    }

    pub fn offer(&mut self, sensor_ts: i64, result: Option<FrameCaptureDetails>) {
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
        if let Some(details) = result {
            producer.offer_capture_result(self.epoch, details, now);
        }
    }

    pub fn result_with(&mut self, sensor_ts: i64, sensor_product: f64, origin: RequestOrigin) -> FrameCaptureDetails {
        self.result_with_flash(
            sensor_ts,
            sensor_product,
            origin,
            camera_contract::flash::FlashResult::UNREPORTED,
        )
    }

    /// Результат кадра с тем, что HAL сообщил о вспышке.
    pub fn result_with_flash(
        &mut self,
        sensor_ts: i64,
        sensor_product: f64,
        origin: RequestOrigin,
        flash: camera_contract::flash::FlashResult,
    ) -> FrameCaptureDetails {
        self.sequence += 1;
        FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(self.sequence),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
                exposure_time: Some(Duration::from_secs_f64(sensor_product / f64::from(ISO))),
                sensitivity_iso: Some(ISO),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState {
                    flash,
                    ..ThreeAResultState::unreported(origin)
                },
            },
            color: CaptureColorHints::default(),
        }
    }

    pub fn presented(&mut self) -> u64 {
        self.backend
            .diagnostics()
            .camera
            .map_or(0, |camera| camera.camera_frames_presented)
    }

    pub fn render(&mut self) -> bool {
        let before = self.presented();
        assert_presented_or_skipped(self.backend.render_and_present(timing(self.render_index)));
        self.render_index += 1;
        self.presented() > before
    }

    /// Яркость плашки на экране (линейный свет экрана, sRGB-основные).
    pub fn display_luminance(&self) -> f64 {
        let [red, green, blue] = self.display_pixel().map(|channel| f64::from(srgb_decode(channel)));
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }

    /// Закодированный пиксель центра изображения видоискателя.
    pub fn display_pixel(&self) -> [f32; 3] {
        let image: ViewfinderReadback = self
            .backend
            .read_viewfinder_image()
            .expect("readback изображения видоискателя")
            .expect("изображение видоискателя есть");
        let pixel = image.pixels[(image.height as usize / 2) * image.width as usize + image.width as usize / 2];
        [pixel[0], pixel[1], pixel[2]]
    }

    /// Движок камеры: замечает новое предложение, ставит его в запрос и публикует указание с `applied`.
    pub fn engine_reacts(&mut self) {
        let Some((generation, proposal)) = self.proposals.newer_than(self.seen) else {
            return;
        };
        self.seen = Some(generation);
        self.commanded_product = proposal.target.seconds_iso();
        self.pending
            .push_back((self.frame + SENSOR_DELAY_FRAMES, self.commanded_product));
        self.first_applied_at.get_or_insert(self.frame);
        self.publish_directive();
    }

    /// Движок камеры объявил предвспышку снимка `shot`: рендер обязан мерить каждый кадр.
    pub fn publish_active_preflash(&self, shot: u64) {
        self.directives.publish(ExposureDirective {
            flash: camera_contract::flash::FlashDirective {
                preflash: camera_contract::flash::PreflashMetering::Active {
                    shot: camera_contract::flash::FlashShotId(shot),
                },
                planned: None,
            },
            ..ExposureDirective::DEFAULT
        });
    }

    pub fn publish_directive(&self) {
        self.directives.publish(ExposureDirective {
            metering: PointOfInterest::WholeFrame,
            lock: self.lock,
            profile: ThreeAProfile::Photo,
            authority: ExposureAuthority::Automatic,
            bias: ExposureBias::NONE,
            applied: ExposureProduct::from_seconds_iso(self.commanded_product)
                .ok()
                .map(|product| AppliedExposure {
                    product,
                    shortfall_ev: 0.0,
                    reach: camera_contract::auto_exposure::ProductReach::Unbounded,
                }),
            stream_start: None,
            flash: camera_contract::flash::FlashDirective::INACTIVE,
        });
    }

    pub fn set_lock(&mut self, lock: AeAfLock) {
        self.lock = lock;
        self.publish_directive();
    }

    /// Экспозиция, дошедшая до кадров к текущему кадру (запрос камеры виден с задержкой конвейера).
    pub fn apply_due_exposure(&mut self) {
        while let Some(&(due, product)) = self.pending.front() {
            if due > self.frame {
                break;
            }
            self.sensor_product = product;
            self.pending.pop_front();
        }
    }

    /// Кадр камеры: экспозиция по задержке, сцена, показ, реакция движка.
    pub fn step(&mut self) -> Shot {
        self.step_with(true)
    }

    /// То же, но результат запроса кадра можно не присылать (`paired = false`): пара не придёт никогда.
    pub fn step_with(&mut self, paired: bool) -> Shot {
        self.apply_due_exposure();
        let sensor_ts = self.timestamp();
        self.insert_scene(sensor_ts, self.sensor_product);
        let result = paired.then(|| self.result_with(sensor_ts, self.sensor_product, RequestOrigin::Repeating));
        self.offer(sensor_ts, result);
        if !paired {
            self.stream
                .producer()
                .expire_pending(Instant::now() + Duration::from_secs(1));
        }
        let shown = self.render();
        let shot = Shot {
            frame: self.frame,
            sensor_product: self.sensor_product,
            display_luminance: shown.then(|| self.display_luminance()),
            pixel: shown.then(|| self.display_pixel()),
        };
        self.history.push(shot);
        self.engine_reacts();
        self.frame += 1;
        shot
    }

    pub fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step();
        }
    }

    pub fn viewfinder(&mut self) -> ViewfinderDiagnostics {
        self.backend.diagnostics().viewfinder.expect("диагностика видоискателя")
    }

    pub fn last_luminance(&self) -> f64 {
        self.history
            .iter()
            .rev()
            .find_map(|shot| shot.display_luminance)
            .expect("был показанный кадр")
    }

    pub fn finish(mut self, journal: &Arc<Journal>) {
        let _ = journal;
        self.stream.producer().end_stream(self.epoch);
        assert_presented_or_skipped(self.backend.render_and_present(timing(self.render_index)));
        assert!(self.backend.detach().is_some(), "окно возвращено");
        let counters = self.stream.counters();
        assert_eq!(counters.leaked_leases, 0, "{counters:?}");
        assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
        assert_no_validation_errors(self.backend.context());
    }
}

/// Экранный средний серый: куда тон ставит `scene_middle_gray`.
pub fn display_middle_gray() -> f64 {
    let settings = ToneCurveSettings::default();
    f64::from(settings.display_middle_gray)
}

/// Что тон даёт на экране для яркости сцены `scene_luminance`.
pub fn tone_display(scene_luminance: f32) -> f64 {
    let curve = ToneCurve::resolve(&ToneCurveSettings::default()).expect("тон");
    f64::from(curve.map_channel(scene_luminance))
}

pub fn ev_error(luminance: f64, wanted: f64) -> f64 {
    (luminance / wanted).log2()
}

pub fn converged_rig() -> Option<(AeRig, Arc<Journal>)> {
    let (mut rig, journal) = rig_or_skip(Some(AutoExposureConfig::default()))?;
    rig.run(80);
    Some((rig, journal))
}
