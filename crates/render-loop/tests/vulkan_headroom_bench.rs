//! Стенд «запас под видео» (V4) через НАСТОЯЩИЙ бэкенд показа: `VulkanDisplayBackend` в режиме камеры
//! на реальном драйвере (RADV, headless-показ, полная validation) + настоящий `RawFrameStream`.
//!
//! Доказывается:
//! * включённый командой стенд записывает КАЖДЫЙ новый кадр камеры (и только новые: перекомпоновка
//!   при 120 Гц его не запускает), а время GPU появляется по проходам с именами;
//! * лизинг каждого кадра возвращается его же сабмитом, на руках у показа не больше одного кадра;
//! * смена потока камеры начинает историю шумодава заново;
//! * объекты стенда создаются лениво (включение само ничего не выделяет), а выключение освобождает их
//!   после fence — живые объекты Vulkan возвращаются ровно к числу до включения;
//! * validation чиста, утечек лизингов нет.

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
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::live_objects::{LiveObjectKind, LiveObjectSnapshot};
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::sync::GpuWaitFence;
use raw_frame_stream::{
    OfferedFrame, ProducerFrame, RawFrameLease, RawFrameStream, RawFrameStreamConfig, ReleaseFences, StreamEpoch,
};
use render_loop::backend::{DisplayBackend, FrameTiming, PresentTarget};
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ViewfinderConfig, WhiteBalanceConfig};
use render_loop::viewfinder::{
    HeadroomBench, HeadroomBenchDiagnostics, ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderDiagnostics,
    ViewfinderExtent,
};
use render_loop::vulkan_backend::{CameraFrameImporter, CameraImportError, SubmissionSerial, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::SurfaceSize;
use support::{Journal, fake_surface};
use vulkan_support::{assert_no_validation_errors, assert_presented_or_skipped, headless_camera_backend_or_skip};

/// Маленький кадр (полезная область = весь кадр): нагрузка стенда того же размера.
const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 96,
    height_px: 160,
};
const BLACK_LEVEL: u16 = 64;
const WHITE_LEVEL: u16 = 1023;
/// Шаг меток сенсора между кадрами (~60 к/с).
const SENSOR_STEP_NS: i64 = 16_666_667;
/// Период показа 120 Гц, нс (метки `FrameTiming` в headless-стенде ни на что не влияют, но честны).
const DISPLAY_PERIOD_120_HZ_NS: i64 = 8_333_333;

fn calibration() -> Vec<camera_contract::capabilities::ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

fn stream_passport() -> Arc<StreamPassport> {
    Arc::new(StreamPassport {
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
    })
}

fn capture_hints() -> CaptureColorHints {
    CaptureColorHints {
        neutral_color_point: Some([500, 1000, 750].map(|numerator| Rational {
            numerator,
            denominator: 1000,
        })),
        ..CaptureColorHints::default()
    }
}

// ---- поддельный производитель и импортёр ----

/// Метки сенсора кадров, возвращённых производителю, в порядке возврата.
#[derive(Default)]
struct Returned(Mutex<Vec<i64>>);

impl Returned {
    fn snapshot(&self) -> Vec<i64> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

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

/// Импортёр: каждый кадр — синтетический RAW16 с серой сценой и лёгкой «текстурой» по метке.
struct GrayImporter {
    context: GpuContext,
    entries: HashMap<i64, (SyntheticRaw16, u64)>,
    counters: ImportCacheCounters,
}

impl CameraFrameImporter for GrayImporter {
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
        // Детерминированный рельеф: демозаику есть что интерполировать, кадры отличаются.
        let shift = u32::try_from(sensor_ts / SENSOR_STEP_NS % 7).unwrap_or(0);
        let texels: Vec<u16> = (0..FRAME_SIZE.height)
            .flat_map(|y| {
                (0..FRAME_SIZE.width)
                    .map(move |x| BLACK_LEVEL + 200 + u16::try_from((x * 3 + y * 5 + shift) % 97).unwrap_or(0))
            })
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

struct Bench {
    backend: VulkanDisplayBackend,
    stream: RawFrameStream,
    returned: Arc<Returned>,
    frame_index: u64,
    next_sequence: i64,
    journal: Arc<Journal>,
}

fn start() -> Option<Bench> {
    let render_config = RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        // Композиция на 120 Гц (прокси UI до сессии 11): запасной период пейсинга и показа — 120 Гц.
        preferred_frame_rate_hz: 120.0,
        // Тест про стенд V4, не про AWB: каждый кадр показывается сразу (без скрытия холодного старта) и
        // считает статистику (один вариант пайплайна ISP — счёт живых объектов не зависит от прореживания).
        viewfinder: ViewfinderConfig {
            white_balance: WhiteBalanceConfig {
                max_hidden_frames: 0,
                statistics_every_n_frames: 1,
                ..WhiteBalanceConfig::default()
            },
            ..ViewfinderConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    // Тест проверяет путь кадра «первый же vsync», а не политику видоискателя: `LatestWins`
    // выдаёт одиночный кадр сразу (буфер с запасом ждал бы второй — см. raw-frame-stream и стенд).
    // Кольцо снимков выключено осознанно: тест про возврат кадров видоискателя, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, и камера получает их назад лишь при вытеснении. Кольцо проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let backend = headless_camera_backend_or_skip(
        &render_config,
        stream.viewfinder(),
        Box::new(|context| {
            Ok(Box::new(GrayImporter {
                context: context.clone(),
                entries: HashMap::new(),
                counters: ImportCacheCounters::default(),
            }))
        }),
    )?;
    Some(Bench {
        backend,
        stream,
        returned: Arc::new(Returned::default()),
        frame_index: 0,
        next_sequence: 1,
        journal: Arc::new(Journal::default()),
    })
}

impl Bench {
    /// Присоединяет surface поколения `generation`.
    fn attach(&mut self, generation: u64) {
        self.backend
            .attach(fake_surface(&self.journal, generation), SURFACE_SIZE)
            .expect("surface присоединяется");
    }

    /// Кадр показа на vsync 120 Гц.
    fn present(&mut self) {
        let expected = MonotonicNanos::from_nanos(
            DISPLAY_PERIOD_120_HZ_NS * i64::try_from(self.frame_index + 1).expect("номер кадра"),
        );
        let timing = FrameTiming {
            frame_index: self.frame_index,
            vsync_sequence: self.frame_index + 1,
            skipped_vsyncs: 0,
            target: PresentTarget::latch_immediately(expected, expected),
        };
        assert_presented_or_skipped(self.backend.render_and_present(timing));
        self.frame_index += 1;
        let outstanding = self.stream.counters().frames_outstanding;
        assert!(
            outstanding <= 1,
            "у показа на руках не больше одного кадра: {outstanding}"
        );
    }

    /// Предлагает новый кадр потока `epoch` с метаданными; возвращает метку сенсора.
    fn offer(&mut self, epoch: StreamEpoch) -> i64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let sensor_ts = 1_000_000_000 + sequence * SENSOR_STEP_NS;
        let producer = self.stream.producer();
        let now = Instant::now();
        producer.offer_frame(
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
        sensor_ts
    }

    /// Камера 60 к/с, экран 120 Гц: новый кадр — и сразу два vsync (новый кадр + перекомпоновка).
    /// Проверяет, что кадр ушёл в ISP первым же vsync и вернулся камере ЭТИМ ЖЕ сабмитом.
    fn camera_frame_at_120_hz(&mut self, epoch: StreamEpoch) {
        let processed_before = self.viewfinder().counters.frames_processed;
        let sensor_ts = self.offer(epoch);
        self.present();
        assert_eq!(
            self.viewfinder().counters.frames_processed,
            processed_before + 1,
            "новый кадр обработан первым же vsync"
        );
        assert_eq!(
            self.returned.snapshot().last(),
            Some(&sensor_ts),
            "кадр возвращён камере своим сабмитом, не дожидаясь следующего"
        );
        self.present();
    }

    /// Несколько vsync без новых кадров: все слоты кольца выданы заново, всё отправленное раньше
    /// завершено — освобождено то, что ждало завершения серий (импорты, нагрузки стенда).
    fn settle(&mut self) {
        for _ in 0..4 {
            self.present();
        }
    }

    fn command(&mut self, command: ViewfinderCommand) {
        assert_eq!(
            self.backend.apply_viewfinder_command(command.clone()),
            ViewfinderCommandOutcome::Applied,
            "{command:?}"
        );
    }

    fn viewfinder(&mut self) -> ViewfinderDiagnostics {
        self.backend.diagnostics().viewfinder.expect("диагностика видоискателя")
    }

    fn headroom(&mut self) -> HeadroomBenchDiagnostics {
        self.viewfinder().headroom
    }
}

/// Живые объекты по видам без fence-ов и семафоров: их число зависит от того, сколько present-fence-ов
/// цепочки показа ещё не переработано (это не стенд), а стенд ни тех, ни других не создаёт.
fn objects_without_presentation_sync(snapshot: &LiveObjectSnapshot) -> Vec<(LiveObjectKind, usize)> {
    LiveObjectKind::ALL
        .into_iter()
        .filter(|kind| !matches!(kind, LiveObjectKind::Fence | LiveObjectKind::Semaphore))
        .map(|kind| (kind, snapshot.count(kind)))
        .collect()
}

/// Сумма объектов по видам из [`objects_without_presentation_sync`].
fn total_count(per_kind: &[(LiveObjectKind, usize)]) -> usize {
    per_kind.iter().map(|(_, count)| count).sum()
}

#[test]
fn headroom_bench_records_every_new_frame_at_120_hz_and_frees_its_objects_on_disable() {
    let Some(mut bench) = start() else {
        return;
    };
    let baseline = bench.backend.context().live_objects();
    bench.attach(1);
    let first_epoch = bench.stream.producer().begin_stream(stream_passport()).epoch;

    // 1. Стенд выключен: видоискатель работает, стенд ничего не создаёт.
    bench.camera_frame_at_120_hz(first_epoch);
    let off = bench.headroom();
    assert_eq!(off.mode, HeadroomBench::Disabled);
    assert_eq!((off.frames_recorded, off.workloads_created), (0, 0));
    // Устойчивое состояние: импорты прошлых кадров освобождены по завершённым сериям.
    bench.settle();
    let before_enable = bench.backend.context().live_objects();

    // 2. Включение само ничего не выделяет (ленивое создание на первом новом кадре).
    bench.command(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled));
    bench.present();
    // Без fence-ов и семафоров: их число зависит от того, сколько present-fence-ов цепочки показа
    // ещё не переработано, и под нагрузкой «плавает» (тест так изредка падал) — стенд их не создаёт.
    assert_eq!(
        objects_without_presentation_sync(&bench.backend.context().live_objects()),
        objects_without_presentation_sync(&before_enable),
        "без нового кадра стенд объектов не создаёт"
    );
    let processed_at_enable = bench.viewfinder().counters.frames_processed;
    const FRAMES_WITH_BENCH: u64 = 4;
    for _ in 0..FRAMES_WITH_BENCH {
        bench.camera_frame_at_120_hz(first_epoch);
    }
    // Ещё vsync без новых кадров: слоты выдаются снова, метки времени стенда собираются.
    for _ in 0..3 {
        bench.present();
    }
    let viewfinder = bench.viewfinder();
    let on = viewfinder.headroom.clone();
    assert_eq!(
        viewfinder.counters.frames_processed - processed_at_enable,
        FRAMES_WITH_BENCH,
        "ISP видоискателя работает как прежде"
    );
    assert_eq!(
        on.frames_recorded, FRAMES_WITH_BENCH,
        "стенд записал ровно новые кадры: {on:?}"
    );
    assert_eq!(on.workload_extent, Some(ViewfinderExtent { width: 64, height: 48 }));
    assert_eq!(on.workloads_created, 1);
    assert_eq!(
        on.history_restarts, 1,
        "история начата первым кадром и дальше смешивается"
    );
    assert_eq!(
        (on.failures, on.sink_busy_skips, on.frames_skipped_no_params),
        (0, 0, 0),
        "{on:?}"
    );
    let pass_names: Vec<&str> = on.pass_times.iter().map(|time| time.pass).collect();
    for expected in ["levels", "temporal_denoise", "output"] {
        assert!(
            pass_names.contains(&expected),
            "нет времени прохода {expected}: {pass_names:?}"
        );
    }
    assert!(
        pass_names.len() >= 11,
        "Menon — 8 проходов + уровни, шумодав, выход: {pass_names:?}"
    );
    let total = on.total.expect("время кадра стенда");
    assert!(total.samples >= 1 && !total.max.is_zero(), "{total:?}");
    assert!(
        total_count(&objects_without_presentation_sync(
            &bench.backend.context().live_objects()
        )) > total_count(&objects_without_presentation_sync(&before_enable)),
        "нагрузка создана"
    );
    assert!(
        viewfinder.counters.recomposed >= FRAMES_WITH_BENCH,
        "на 120 Гц каждый второй vsync — перекомпоновка без ISP: {:?}",
        viewfinder.counters
    );

    // 3. Новая сессия камеры (тот же размер): история шумодава начинается заново.
    bench.stream.producer().end_stream(first_epoch);
    bench.present();
    let second_epoch = bench.stream.producer().begin_stream(stream_passport()).epoch;
    bench.camera_frame_at_120_hz(second_epoch);
    bench.camera_frame_at_120_hz(second_epoch);
    let changed = bench.headroom();
    assert_eq!(changed.stream_changes, 1, "{changed:?}");
    assert_eq!(
        changed.history_restarts, 2,
        "первый кадр нового потока не смешан со старым"
    );
    assert_eq!(changed.frames_recorded, FRAMES_WITH_BENCH + 2);
    assert_eq!(
        changed.workloads_created, 1,
        "размер тот же — нагрузка переиспользуется"
    );

    // 4. Выключение: объекты освобождаются после fence своих сабмитов, без ожиданий GPU.
    bench.command(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled));
    bench.settle();
    let disabled = bench.headroom();
    assert_eq!(disabled.mode, HeadroomBench::Disabled);
    assert_eq!(disabled.workload_extent, None);
    assert_eq!(disabled.workloads_awaiting_release, 0, "{disabled:?}");
    assert_eq!(disabled.workloads_released, disabled.workloads_created);
    assert_eq!(
        objects_without_presentation_sync(&bench.backend.context().live_objects()),
        objects_without_presentation_sync(&before_enable),
        "живые объекты вернулись к числу до включения"
    );
    // Выключенный стенд новые кадры не трогает, счётчики и окна сохраняются для отчёта.
    bench.camera_frame_at_120_hz(second_epoch);
    let after = bench.headroom();
    assert_eq!(after.frames_recorded, FRAMES_WITH_BENCH + 2);
    assert!(!after.pass_times.is_empty());

    // 5. Остановка и отсоединение без утечек.
    bench.stream.producer().end_stream(second_epoch);
    bench.present();
    assert!(bench.backend.detach().is_some(), "окно возвращено");
    let counters = bench.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(bench.returned.snapshot().len(), 8, "все кадры возвращены производителю");
    assert_eq!(
        bench.backend.context().live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    assert_no_validation_errors(bench.backend.context());
}

#[test]
fn enabled_bench_is_released_with_the_surface_and_recreated_lazily_on_the_next_one() {
    let Some(mut bench) = start() else {
        return;
    };
    let baseline = bench.backend.context().live_objects();
    bench.attach(1);
    let epoch = bench.stream.producer().begin_stream(stream_passport()).epoch;
    bench.command(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled));
    bench.camera_frame_at_120_hz(epoch);
    assert_eq!(bench.headroom().frames_recorded, 1);

    // Surface уходит с включённым стендом: GPU дождались, всё освобождено, лизинги возвращены.
    assert!(bench.backend.detach().is_some());
    let detached = bench.headroom();
    assert_eq!(detached.mode, HeadroomBench::Enabled, "настройка переживает surface");
    assert_eq!(detached.workload_extent, None);
    assert_eq!(detached.workloads_released, detached.workloads_created);
    assert_eq!(bench.backend.context().live_objects(), baseline);

    // Новая surface: стенд по-прежнему включён и создаёт нагрузку на первом новом кадре.
    bench.attach(2);
    bench.camera_frame_at_120_hz(epoch);
    let again = bench.headroom();
    assert_eq!(again.frames_recorded, 2, "счётчики копятся за жизнь бэкенда");
    assert_eq!(again.workloads_created, 2);
    bench.stream.producer().end_stream(epoch);
    bench.present();
    assert!(bench.backend.detach().is_some());
    assert_eq!(bench.stream.counters().leaked_leases, 0);
    assert_eq!(bench.backend.context().live_objects(), baseline);
    assert_no_validation_errors(bench.backend.context());
}
