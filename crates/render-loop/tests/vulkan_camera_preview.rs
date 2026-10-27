//! Функциональный тест показа кадров камеры: НАСТОЯЩИЙ поток рендера + `VulkanDisplayBackend` в режиме
//! камеры на реальном драйвере (RADV, headless-показ, полная validation) + настоящий `RawFrameStream` и
//! поддельный производитель. Импорт буфера в GPU подменён тестовым импортёром (на ПК нет
//! `AHardwareBuffer`): он создаёт обычные картинки Vulkan с известным RAW-узором. Всё остальное — боевой
//! путь: опрос видоискателя, сабмит с ожиданием/экспортом fence, учёт лизингов, возврат камере.
//!
//! Доказывается: кадры идут производитель -> поток рендера -> показ; каждый лизинг возвращается
//! производителю — и никогда раньше, чем сабмит, переставший читать его буфер; после остановки и отсоединения
//! возвращено всё, `leaked_leases == 0`, объекты Vulkan вернулись к исходным, validation чиста.
//!
//! Ограничение стенда: у headless-показа нет чтения показанных картинок; корректность пикселей предпросмотра
//! доказывают паритетные тесты `gpu-vulkan` (шейдер против CPU-эталона).

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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::sync::{FenceWait, GpuReleaseFence, GpuWaitFence, wait_release_fence_on_cpu};
use raw_frame_stream::{
    OfferedFrame, ProducerFrame, RawFrameLease, RawFrameStream, RawFrameStreamConfig, ReleaseFences, StreamEpoch,
};
use render_loop::config::RenderLoopConfig;
use render_loop::mailbox::InboxDelivery;
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts, RenderState};
use render_loop::vulkan_backend::{CameraFrameImporter, CameraImportError, SubmissionSerial};
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};
use support::{FakeCommands, Journal as WindowJournal, fake_surface, wait_until};
use vulkan_support::{assert_no_validation_errors, headless_camera_backend_or_skip, vsync_clock_at_120_hz};

const DEADLINE: Duration = Duration::from_secs(15);
const FENCE_TIMEOUT: Duration = Duration::from_secs(5);
const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 192,
    height_px: 108,
};
/// Кадр сенсора в тесте: маленький, но не квадрат и не степень двойки.
const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
/// Период кадров производителя (~60 к/с).
const PRODUCER_PERIOD: Duration = Duration::from_millis(16);
/// Шаг меток сенсора между кадрами, наносекунды.
const SENSOR_STEP_NANOS: i64 = 16_666_667;

// ---- журнал событий: рисования импортёра и возвраты производителю в одном порядке ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// Импортёр отдал буфер кадра `sensor_ts` сабмиту серии `serial` (он будет читать этот буфер).
    Draw { sensor_ts: i64, serial: u64 },
    /// Производитель получил буфер кадра `sensor_ts` назад.
    Return { sensor_ts: i64 },
}

#[derive(Default)]
struct EventLog(Mutex<Vec<Event>>);

impl EventLog {
    fn push(&self, event: Event) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }

    fn snapshot(&self) -> Vec<Event> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Что вернули производителю: метка и fence-ы (для проверки сигнала).
struct ReturnedBuffer {
    sensor_ts: i64,
    fences: Vec<GpuReleaseFence>,
}

#[derive(Default)]
struct ProducerBook {
    returned: Mutex<Vec<ReturnedBuffer>>,
}

impl ProducerBook {
    fn returned_count(&self) -> usize {
        self.returned.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

// ---- поддельный кадр производителя ----

struct FakeFrame {
    sensor_ts: i64,
    /// Адрес для `hardware_buffer`: настоящего AHB на ПК нет, тестовый импортёр его не разыменовывает.
    storage: Box<u8>,
    log: Arc<EventLog>,
    book: Arc<ProducerBook>,
}

// SAFETY: указатель из `hardware_buffer` указывает на `storage`, живой до `return_to_producer` (контракт трейта).
unsafe impl ProducerFrame for FakeFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        let pointer = NonNull::from(&*self.storage).cast::<c_void>();
        // SAFETY: `storage` живёт, пока жив `self`; заимствование ограничено `&self`.
        unsafe { BorrowedHardwareBuffer::from_raw(pointer) }
    }

    fn return_to_producer(self: Box<Self>, release: ReleaseFences) {
        self.log.push(Event::Return {
            sensor_ts: self.sensor_ts,
        });
        self.book
            .returned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(ReturnedBuffer {
                sensor_ts: self.sensor_ts,
                fences: release.into_vec(),
            });
    }
}

// ---- тестовый импортёр: обычные картинки Vulkan вместо AHardwareBuffer ----

struct TestImportEntry {
    source: SyntheticRaw16,
    last_use: u64,
}

struct TestImporter {
    context: GpuContext,
    log: Arc<EventLog>,
    alive: Arc<AtomicUsize>,
    entries: HashMap<i64, TestImportEntry>,
    counters: ImportCacheCounters,
}

impl CameraFrameImporter for TestImporter {
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError> {
        let sensor_ts = lease.passport().sensor_timestamp.as_nanos();
        self.log.push(Event::Draw {
            sensor_ts,
            serial: submission.0,
        });
        if let Some(entry) = self.entries.get_mut(&sensor_ts) {
            entry.last_use = submission.0;
            self.counters.hits += 1;
            return Ok(entry.source.source());
        }
        self.counters.misses += 1;
        // Узор зависит от метки кадра: соседние кадры различаются.
        let seed = u32::try_from(sensor_ts.rem_euclid(1_000)).unwrap_or(0);
        let texels: Vec<u16> = (0..FRAME_SIZE.height)
            .flat_map(|y| (0..FRAME_SIZE.width).map(move |x| ((x * 13 + y * 29 + seed) % 1_100) as u16))
            .collect();
        let source = SyntheticRaw16::new(&self.context, FRAME_SIZE.width, FRAME_SIZE.height, &texels)?;
        let mosaic = source.source();
        self.entries.insert(
            sensor_ts,
            TestImportEntry {
                source,
                last_use: submission.0,
            },
        );
        self.alive.store(self.entries.len(), Ordering::SeqCst);
        Ok(mosaic)
    }

    fn retire_completed(&mut self, completed: SubmissionSerial) {
        self.entries.retain(|_, entry| entry.last_use > completed.0);
        self.alive.store(self.entries.len(), Ordering::SeqCst);
    }

    fn release_all_imports_gpu_idle(&mut self) {
        self.entries.clear();
        self.alive.store(0, Ordering::SeqCst);
    }

    fn imports_alive(&self) -> usize {
        self.entries.len()
    }

    fn cache_counters(&self) -> ImportCacheCounters {
        self.counters
    }
}

// ---- стенд ----

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
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с допустимо"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
    }
}

/// Поток-«камера»: предлагает кадры, пока не остановят, и не превышает пул буферов.
struct Camera {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<u64>>,
}

impl Camera {
    fn start(
        stream: &RawFrameStream,
        log: &Arc<EventLog>,
        book: &Arc<ProducerBook>,
    ) -> (Self, StreamEpoch, Arc<AtomicUsize>) {
        let producer = stream.producer();
        let started = producer.begin_stream(Arc::new(stream_passport()));
        let epoch = started.epoch;
        let max_images = usize::try_from(started.pool.reader_max_images()).expect("пул помещается в usize");
        let offered = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_offered = Arc::clone(&offered);
        let (log, book) = (Arc::clone(log), Arc::clone(book));
        let join = thread::Builder::new()
            .name("test-camera".to_owned())
            .spawn(move || {
                let mut sequence = 0_i64;
                while !thread_stop.load(Ordering::SeqCst) {
                    // Камера не отдаёт больше буферов, чем в пуле (иначе её reader отказал бы).
                    let outstanding = thread_offered
                        .load(Ordering::SeqCst)
                        .saturating_sub(book.returned_count());
                    if outstanding < max_images {
                        let sensor_ts = 1_000_000_000 + sequence * SENSOR_STEP_NANOS;
                        let now = Instant::now();
                        let frame = OfferedFrame {
                            sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
                            acquire_fence: GpuWaitFence::already_signaled(),
                            frame: Box::new(FakeFrame {
                                sensor_ts,
                                storage: Box::new(0),
                                log: Arc::clone(&log),
                                book: Arc::clone(&book),
                            }),
                        };
                        producer.offer_frame(epoch, frame, now);
                        thread_offered.fetch_add(1, Ordering::SeqCst);
                        producer.offer_capture_result(epoch, result_for(sensor_ts, sequence), now);
                        sequence += 1;
                    }
                    thread::sleep(PRODUCER_PERIOD);
                }
                u64::try_from(sequence).unwrap_or(0)
            })
            .expect("поток камеры");
        (Self { stop, join: Some(join) }, epoch, offered)
    }

    /// Останавливает предложение кадров; возвращает сколько кадров предложено.
    fn stop_offering(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            join.join().expect("поток камеры не упал");
        }
    }
}

fn result_for(sensor_ts: i64, sequence: i64) -> FrameCaptureDetails {
    FrameCaptureDetails {
        result: CaptureResultMeta {
            frame_number: FrameNumber(sequence + 500),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
            exposure_time: None,
            sensitivity_iso: Some(100),
            reported_frame_duration: None,
            rolling_shutter_skew: None,
        },
        color: CaptureColorHints::default(),
    }
}

struct Rig {
    handle: RenderLoopHandle,
    windows: Arc<WindowJournal>,
    context: GpuContext,
    stream: RawFrameStream,
    log: Arc<EventLog>,
    book: Arc<ProducerBook>,
    imports_alive: Arc<AtomicUsize>,
}

fn start_rig() -> Option<Rig> {
    let render_config = RenderLoopConfig {
        diagnostics_poll_interval_ms: 20,
        ..RenderLoopConfig::default()
    };
    let stream = RawFrameStream::new(RawFrameStreamConfig::default());
    let log = Arc::new(EventLog::default());
    let imports_alive = Arc::new(AtomicUsize::new(0));
    let (factory_log, factory_alive) = (Arc::clone(&log), Arc::clone(&imports_alive));
    let backend = headless_camera_backend_or_skip(
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
    )?;
    let context = backend.context().clone();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: render_config,
        backend,
        clock: vsync_clock_at_120_hz(),
        commands: Arc::new(FakeCommands::default()),
    })
    .expect("поток запускается");
    Some(Rig {
        handle,
        windows: Arc::new(WindowJournal::default()),
        context,
        stream,
        log,
        book: Arc::new(ProducerBook::default()),
        imports_alive,
    })
}

impl Rig {
    fn show_surface(&self, generation: u64) {
        let inbox = self.handle.inbox();
        assert_eq!(
            inbox.surface_created(fake_surface(&self.windows, generation)),
            InboxDelivery::Delivered
        );
        inbox.surface_changed(SurfaceGeneration::new(generation), SURFACE_SIZE);
    }

    fn destroy_surface(&self, generation: u64) {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        self.handle.inbox().surface_destroyed(request);
        assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    }

    fn camera_presented(&self) -> u64 {
        self.handle
            .status()
            .backend
            .camera
            .map_or(0, |camera| camera.camera_frames_presented)
    }

    /// Все возвраты производителю удовлетворяют правилам безопасности.
    ///
    /// `rules` выбирает строгость: правило «возврат только после более позднего сабмита» применимо, когда
    /// surface не отсоединялась посреди показа (возвраты при отсоединении идут по простою GPU).
    fn assert_returns_were_safe(&self, rules: ReturnRules) {
        let events = self.log.snapshot();
        for (position, event) in events.iter().enumerate() {
            let Event::Return { sensor_ts } = *event else { continue };
            // 1. Буфер не читается после возврата.
            assert!(
                !events[position + 1..]
                    .iter()
                    .any(|later| matches!(later, Event::Draw { sensor_ts: drawn, .. } if *drawn == sensor_ts)),
                "кадр {sensor_ts} рисовался ПОСЛЕ возврата производителю"
            );
        }
        // 2. Возврат во время показа идёт только после более позднего сабмита, чем последнее чтение буфера.
        //    (Возвраты при остановке/отсоединении идут по простою GPU и этому правилу не подчиняются.)
        if rules == ReturnRules::NoReadAfterReturn {
            return self.assert_all_fences_signal();
        }
        let streaming_end = events
            .iter()
            .rposition(|event| matches!(event, Event::Draw { .. }))
            .unwrap_or(0);
        for (position, event) in events.iter().enumerate() {
            let Event::Return { sensor_ts } = *event else { continue };
            if position > streaming_end {
                continue;
            }
            let last_read = events[..position]
                .iter()
                .filter_map(|earlier| match earlier {
                    Event::Draw {
                        sensor_ts: drawn,
                        serial,
                    } if *drawn == sensor_ts => Some(*serial),
                    _ => None,
                })
                .max();
            let latest_submit = events[..position]
                .iter()
                .filter_map(|earlier| match earlier {
                    Event::Draw { serial, .. } => Some(*serial),
                    Event::Return { .. } => None,
                })
                .max();
            if let Some(last_read) = last_read {
                assert!(
                    latest_submit.is_some_and(|latest| latest > last_read),
                    "кадр {sensor_ts} возвращён, хотя позднее сабмита {last_read} других сабмитов не было"
                );
            }
        }
        self.assert_all_fences_signal();
    }

    /// Каждый fence, с которым буфер вернулся производителю, срабатывает (GPU закончил с буфером).
    fn assert_all_fences_signal(&self) {
        let returned = self.book.returned.lock().unwrap_or_else(PoisonError::into_inner);
        for buffer in returned.iter() {
            for fence in &buffer.fences {
                assert_eq!(
                    wait_release_fence_on_cpu(&self.context, fence, FENCE_TIMEOUT).expect("ожидание fence"),
                    FenceWait::Signaled,
                    "fence кадра {} не сработал",
                    buffer.sensor_ts
                );
            }
        }
    }
}

/// Какие правила возвратов проверять.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReturnRules {
    /// Все правила, включая «возврат только после более позднего сабмита».
    Full,
    /// Только «не читается после возврата» и сигнал fence-ов (surface отсоединялась посреди показа).
    NoReadAfterReturn,
}

#[test]
fn frames_flow_to_present_and_every_lease_returns_safely() {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.camera_presented() >= 30),
        "кадры камеры не дошли до показа: {:?}",
        rig.handle.status()
    );
    let first_sequence = rig
        .handle
        .status()
        .backend
        .camera
        .and_then(|camera| camera.last_presented_frame)
        .expect("последний показанный кадр известен")
        .sequence;
    assert!(
        wait_until(DEADLINE, || {
            rig.handle
                .status()
                .backend
                .camera
                .and_then(|camera| camera.last_presented_frame)
                .is_some_and(|frame| frame.sequence > first_sequence + 5)
        }),
        "последовательность показанных кадров не растёт"
    );

    // Пока идёт показ: лизинги уже возвращаются (по fence более позднего сабмита), камера не голодает.
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() >= 10),
        "производитель не получает буферы назад: {:?}",
        rig.handle.status().backend.camera
    );
    let streaming = rig.handle.status();
    let diagnostics = streaming.backend.camera.clone().expect("диагностика камеры");
    assert_eq!(streaming.state, RenderState::Presenting);
    assert_eq!(streaming.backend_errors, 0, "{streaming:?}");
    assert!(diagnostics.camera_frames_imported >= diagnostics.camera_frames_presented);
    assert!(
        diagnostics.import_cache_misses >= 30,
        "каждый новый кадр — настоящий импорт: {diagnostics:?}"
    );
    assert!(diagnostics.release_fences_exported > 0, "{diagnostics:?}");
    assert_eq!(diagnostics.release_export_failures, 0, "{diagnostics:?}");
    assert!(
        diagnostics.present_history.len() >= 2
            && diagnostics
                .present_history
                .windows(2)
                .all(|pair| pair[0].sensor_timestamp_nanos < pair[1].sensor_timestamp_nanos),
        "история показов: метки сенсора растут: {:?}",
        diagnostics.present_history
    );
    rig.assert_returns_were_safe(ReturnRules::Full);

    // Остановка камеры: поток рендера видит StreamInactive, рисует чёрный и возвращает показанный кадр.
    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() == offered_total),
        "после остановки возвращено {} из {offered_total}: {:?}",
        rig.book.returned_count(),
        rig.stream.counters()
    );
    assert!(
        wait_until(DEADLINE, || rig.imports_alive.load(Ordering::SeqCst) == 0),
        "импорты не освобождены после остановки потока"
    );
    let presented_after_stop = rig.camera_presented();

    rig.destroy_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_leaked, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    assert_eq!(
        rig.camera_presented(),
        presented_after_stop,
        "после остановки новые кадры не показываются"
    );
    rig.assert_returns_were_safe(ReturnRules::Full);
    assert_eq!(
        rig.context.live_objects(),
        baseline,
        "объекты Vulkan вернулись к исходным"
    );
    let final_status = rig.handle.status();
    assert_eq!(final_status.backend.validation_errors, 0);
    assert_eq!(final_status.backend.detach_failures, 0);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn destroying_the_surface_mid_stream_returns_every_lease_without_leaks() {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    rig.show_surface(1);
    assert!(
        wait_until(DEADLINE, || rig.camera_presented() >= 15),
        "кадры не дошли до показа"
    );
    // Surface уничтожается, пока камера продолжает слать кадры и кадр лежит на экране.
    rig.destroy_surface(1);
    assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(
        wait_until(DEADLINE, || rig.book.returned_count() == offered_total),
        "возвращено {} из {offered_total}: {:?}",
        rig.book.returned_count(),
        rig.stream.counters()
    );
    let counters = rig.stream.counters();
    assert_eq!(counters.leaked_leases, 0, "{counters:?}");
    assert_eq!(counters.frames_outstanding, 0, "{counters:?}");
    let final_status = rig.handle.status();
    let camera_diagnostics = final_status.backend.camera.expect("диагностика камеры");
    assert!(
        camera_diagnostics.leases_released_after_idle >= 1,
        "показанный кадр возвращён по простою GPU при отсоединении: {camera_diagnostics:?}"
    );
    assert_eq!(
        camera_diagnostics.leases_leaked_on_teardown, 0,
        "{camera_diagnostics:?}"
    );
    rig.assert_returns_were_safe(ReturnRules::Full);
    assert_eq!(rig.context.live_objects(), baseline);
    assert_eq!(final_status.backend.validation_errors, 0);
    assert_no_validation_errors(&rig.context);
}

#[test]
fn surface_cycles_keep_counters_cumulative_and_leak_nothing() {
    let Some(rig) = start_rig() else { return };
    let baseline = rig.context.live_objects();
    let (mut camera, epoch, offered) = Camera::start(&rig.stream, &rig.log, &rig.book);

    let mut presented_before = 0;
    for generation in 1..=3 {
        rig.show_surface(generation);
        assert!(
            wait_until(DEADLINE, || rig.camera_presented() >= presented_before + 8),
            "цикл {generation}: кадры не дошли до показа ({presented_before} уже было)"
        );
        presented_before = rig.camera_presented();
        rig.destroy_surface(generation);
        assert!(wait_until(DEADLINE, || rig.handle.status().display.is_none()));
        assert!(
            rig.camera_presented() >= presented_before,
            "счётчики показа не обнуляются при отсоединении surface"
        );
    }

    camera.stop_offering();
    rig.stream.producer().end_stream(epoch);
    let offered_total = offered.load(Ordering::SeqCst);
    assert!(wait_until(DEADLINE, || rig.book.returned_count() == offered_total));
    assert_eq!(rig.stream.counters().leaked_leases, 0);
    rig.assert_returns_were_safe(ReturnRules::NoReadAfterReturn);
    assert_eq!(rig.context.live_objects(), baseline);
    assert_no_validation_errors(&rig.context);
}
