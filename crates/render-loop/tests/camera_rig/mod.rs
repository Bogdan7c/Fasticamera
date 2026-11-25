//! Стенд функциональных тестов показа камеры (`vulkan_camera_preview.rs`): поддельный производитель
//! кадров, тестовый импортёр, поток-«камера», поток рендера с бэкендом камеры и проверки возвратов.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::support::{FakeCommands, Journal as WindowJournal, fake_surface, wait_until};
use super::vulkan_support::vsync_clock_at_120_hz;
use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Rational, RawSupport, SensorLevels, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::{camera_id, main_wide_camera};
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
use render_loop::thread::{RenderLoop, RenderLoopHandle, RenderLoopParts};
use render_loop::viewfinder::{
    AwbStatisticsReader, ViewfinderCommand, ViewfinderCommandOutcome, ViewfinderDiagnostics, ViewfinderPainter,
};
use render_loop::vulkan_backend::{CameraFrameImporter, CameraImportError, SubmissionSerial};
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::surface::{SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff};

pub const DEADLINE: Duration = Duration::from_secs(15);
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(5);
pub const SURFACE_SIZE: SurfaceSize = SurfaceSize {
    width_px: 192,
    height_px: 108,
};
/// Кадр сенсора в тесте: маленький, но не квадрат и не степень двойки.
pub const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
/// Период кадров производителя (~60 к/с).
pub const PRODUCER_PERIOD: Duration = Duration::from_millis(16);
/// Шаг меток сенсора между кадрами, наносекунды.
pub const SENSOR_STEP_NANOS: i64 = 16_666_667;

// ---- журнал событий: рисования импортёра и возвраты производителю в одном порядке ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Импортёр отдал буфер кадра `sensor_ts` сабмиту серии `serial` (он будет читать этот буфер).
    Draw { sensor_ts: i64, serial: u64 },
    /// Производитель получил буфер кадра `sensor_ts` назад.
    Return { sensor_ts: i64 },
}

#[derive(Default)]
pub struct EventLog(Mutex<Vec<Event>>);

impl EventLog {
    pub fn push(&self, event: Event) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }

    pub fn snapshot(&self) -> Vec<Event> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Что вернули производителю: метка и fence-ы (для проверки сигнала).
pub struct ReturnedBuffer {
    pub sensor_ts: i64,
    pub fences: Vec<GpuReleaseFence>,
}

#[derive(Default)]
pub struct ProducerBook {
    pub returned: Mutex<Vec<ReturnedBuffer>>,
}

impl ProducerBook {
    pub fn returned_count(&self) -> usize {
        self.returned.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

// ---- поддельный кадр производителя ----

pub struct FakeFrame {
    pub sensor_ts: i64,
    /// Адрес для `hardware_buffer`: настоящего AHB на ПК нет, тестовый импортёр его не разыменовывает.
    pub storage: Box<u8>,
    pub log: Arc<EventLog>,
    pub book: Arc<ProducerBook>,
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

pub struct TestImportEntry {
    pub source: SyntheticRaw16,
    pub last_use: u64,
}

pub struct TestImporter {
    pub context: GpuContext,
    pub log: Arc<EventLog>,
    pub alive: Arc<AtomicUsize>,
    pub entries: HashMap<i64, TestImportEntry>,
    pub counters: ImportCacheCounters,
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
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с допустимо"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        // Калибровка цвета образца основной камеры: без неё ISP честно отказался бы считать цвет.
        color_calibration: fixture_calibration(),
    }
}

pub fn fixture_calibration() -> Vec<ColorCalibrationSet> {
    match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    }
}

/// Нейтраль HAL (до своего AWB сессии 08 ISP берёт её из метаданных кадра).
pub fn neutral() -> [Rational; 3] {
    [
        Rational {
            numerator: 1,
            denominator: 2,
        },
        Rational {
            numerator: 1,
            denominator: 1,
        },
        Rational {
            numerator: 3,
            denominator: 4,
        },
    ]
}

/// Поток-«камера»: предлагает кадры, пока не остановят, и не превышает пул буферов.
pub struct Camera {
    pub stop: Arc<AtomicBool>,
    pub join: Option<JoinHandle<u64>>,
}

impl Camera {
    pub fn start(
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
    pub fn stop_offering(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            join.join().expect("поток камеры не упал");
        }
    }
}

pub fn result_for(sensor_ts: i64, sequence: i64) -> FrameCaptureDetails {
    FrameCaptureDetails {
        result: CaptureResultMeta {
            frame_number: FrameNumber(sequence + 500),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(sensor_ts)),
            exposure_time: None,
            sensitivity_iso: Some(100),
            reported_frame_duration: None,
            rolling_shutter_skew: None,
            three_a: camera_contract::three_a::ThreeAResultState::unreported(
                camera_contract::three_a::RequestOrigin::Repeating,
            ),
        },
        color: CaptureColorHints {
            neutral_color_point: Some(neutral()),
            ..CaptureColorHints::default()
        },
    }
}

pub struct Rig {
    pub handle: RenderLoopHandle,
    pub windows: Arc<WindowJournal>,
    pub context: GpuContext,
    pub stream: RawFrameStream,
    pub log: Arc<EventLog>,
    pub book: Arc<ProducerBook>,
    pub imports_alive: Arc<AtomicUsize>,
    pub awb: AwbStatisticsReader,
}

pub fn start_rig() -> Option<Rig> {
    start_rig_with_policy(default_unchanged_policy())
}

/// Политика пропуска тиков стенда по умолчанию: второй режим набора (`FASTICAMERA_TEST_UNCHANGED_POLICY=skip`)
/// переводит под пропуск тиков и стенды, которые политику сами не выбирают.
pub fn default_unchanged_policy() -> render_loop::config::UnchangedFramePolicy {
    if std::env::var("FASTICAMERA_TEST_UNCHANGED_POLICY").as_deref() == Ok("skip") {
        render_loop::config::UnchangedFramePolicy::SkipWhenUnchanged
    } else {
        render_loop::config::UnchangedFramePolicy::AlwaysRedraw
    }
}

/// Стенд с заданной политикой пропуска тиков без изменений (решение 214): тот же поток рендера, бэкенд и камера.
/// Бэкенд берётся без подмены политики окружением: политика — ровно `policy`.
pub fn start_rig_with_policy(policy: render_loop::config::UnchangedFramePolicy) -> Option<Rig> {
    start_rig_wired(policy, AutoExposureWiring::Off)
}

/// Стенд с нашим AE на потоке рендера. Статистика кадра нужна только AE (без него стадия не считается, ленты
/// публикаций пусты), поэтому тесты, которые проверяют публикации статистики, берут этот стенд, а не прежний.
pub fn start_rig_with_auto_exposure(policy: render_loop::config::UnchangedFramePolicy) -> Option<Rig> {
    start_rig_wired(policy, AutoExposureWiring::On)
}

/// Подключён ли к стенду наш AE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoExposureWiring {
    /// Вендорский AE: потребителей статистики кадра нет.
    Off,
    /// Наш AE.
    On,
}

fn start_rig_wired(
    policy: render_loop::config::UnchangedFramePolicy,
    auto_exposure: AutoExposureWiring,
) -> Option<Rig> {
    let render_config = RenderLoopConfig {
        diagnostics_poll_interval_ms: 20,
        unchanged_frame: render_loop::config::UnchangedFrameConfig {
            policy,
            ..render_loop::config::UnchangedFrameConfig::default()
        },
        ..RenderLoopConfig::default()
    };
    // Кольцо снимков выключено осознанно: тест про возврат кадров видоискателя, а кольцо (по умолчанию 12 кадров,
    // решение 139) держит доли кадров, и камера получает их назад лишь при вытеснении. Кольцо проверяют `vulkan_still_*.rs`.
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        snapshot_ring_max_frames: 0,
        ..RawFrameStreamConfig::default()
    });
    let log = Arc::new(EventLog::default());
    let imports_alive = Arc::new(AtomicUsize::new(0));
    let (factory_log, factory_alive) = (Arc::clone(&log), Arc::clone(&imports_alive));
    let backend = super::vulkan_support::headless_camera_backend_keeping_unchanged_policy_or_skip(
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
        render_loop::viewfinder::WhiteLinks::default(),
    )?;
    let backend = match auto_exposure {
        AutoExposureWiring::Off => backend,
        AutoExposureWiring::On => backend.with_auto_exposure(render_loop::viewfinder::AutoExposureLinks {
            directives: camera_contract::three_a::DirectiveSlot::new(),
            proposals: camera_contract::auto_exposure::ExposureProposalChannel::new(),
        }),
    };
    let context = backend.context().clone();
    let awb = backend.awb_statistics();
    let handle = RenderLoop::spawn(RenderLoopParts {
        config: render_config,
        backend,
        clock: vsync_clock_at_120_hz(),
        commands: Arc::new(FakeCommands::default()),
        performance_hints: std::sync::Arc::new(shell_contract::performance_hint::NoPerformanceHints::new(
            "тест без ADPF",
        )),
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
        awb,
    })
}

impl Rig {
    pub fn show_surface(&self, generation: u64) {
        let inbox = self.handle.inbox();
        assert_eq!(
            inbox.surface_created(fake_surface(&self.windows, generation)),
            InboxDelivery::Delivered
        );
        inbox.surface_changed(SurfaceGeneration::new(generation), SURFACE_SIZE);
    }

    pub fn destroy_surface(&self, generation: u64) {
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(generation));
        self.handle.inbox().surface_destroyed(request);
        assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    }

    /// Шлёт команду видоискателю и ждёт её итога в статусе.
    pub fn command(&self, command: ViewfinderCommand, expected: ViewfinderCommandOutcome) {
        let before = self.handle.status().viewfinder_commands;
        let handled_before = before.applied + before.already_in_effect + before.rejected + before.unsupported;
        assert_eq!(
            self.handle.inbox().viewfinder_command(command.clone()),
            InboxDelivery::Delivered
        );
        assert!(
            wait_until(DEADLINE, || {
                let log = self.handle.status().viewfinder_commands;
                log.applied + log.already_in_effect + log.rejected + log.unsupported > handled_before
            }),
            "команда {command:?} не обработана"
        );
        let last = self
            .handle
            .status()
            .viewfinder_commands
            .last
            .expect("последняя команда");
        assert_eq!((last.command, last.outcome), (command, expected));
    }

    /// Выбирает рисовальщика (до показа; повтор текущего — «уже действует»).
    pub fn set_painter(&self, painter: ViewfinderPainter) {
        let expected = if painter == ViewfinderPainter::Isp {
            ViewfinderCommandOutcome::AlreadyInEffect
        } else {
            ViewfinderCommandOutcome::Applied
        };
        self.command(ViewfinderCommand::SetPainter(painter), expected);
    }

    /// Диагностика видоискателя (ждёт первого опроса диагностики после старта потока).
    pub fn viewfinder(&self) -> ViewfinderDiagnostics {
        assert!(
            wait_until(DEADLINE, || self.handle.status().backend.viewfinder.is_some()),
            "бэкенд камеры не отдаёт диагностику видоискателя"
        );
        self.handle
            .status()
            .backend
            .viewfinder
            .expect("диагностика видоискателя появилась")
    }

    pub fn camera_presented(&self) -> u64 {
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
    pub fn assert_returns_were_safe(&self, rules: ReturnRules) {
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
        // 2. ISP: каждый кадр импортируется (читается) ровно одним сабмитом и возвращается сразу после
        //    него с fence этого сабмита — к импорту следующего кадра на руках у показа не больше одного.
        //    (Возвраты при остановке/отсоединении идут по простою GPU и этому правилу не подчиняются.)
        match rules {
            ReturnRules::NoReadAfterReturn => return self.assert_all_fences_signal(),
            ReturnRules::DebugMosaic => {
                self.assert_mosaic_returns_follow_a_later_submit(&events);
                return self.assert_all_fences_signal();
            }
            ReturnRules::Full => {}
        }
        let mut draws_per_frame: HashMap<i64, usize> = HashMap::new();
        let mut on_hand: Vec<i64> = Vec::new();
        let mut max_on_hand = 0;
        for event in &events {
            match *event {
                Event::Draw { sensor_ts, .. } => {
                    *draws_per_frame.entry(sensor_ts).or_default() += 1;
                    on_hand.push(sensor_ts);
                    max_on_hand = max_on_hand.max(on_hand.len());
                }
                Event::Return { sensor_ts } => on_hand.retain(|held| *held != sensor_ts),
            }
        }
        assert!(
            draws_per_frame.values().all(|draws| *draws == 1),
            "кадр импортирован больше одного раза (перекомпоновка не должна импортировать): {draws_per_frame:?}"
        );
        assert!(
            max_on_hand <= 1,
            "у показа одновременно {max_on_hand} кадров: кадр ISP обязан вернуться своим же сабмитом"
        );
        self.assert_all_fences_signal();
    }

    /// Мозаика: кадр на экране перерисовывается, поэтому возврат во время показа идёт только после
    /// более позднего сабмита, чем последнее чтение буфера (fence позднего сабмита покрывает ранние).
    pub fn assert_mosaic_returns_follow_a_later_submit(&self, events: &[Event]) {
        let streaming_end = events
            .iter()
            .rposition(|event| matches!(event, Event::Draw { .. }))
            .unwrap_or(0);
        for (position, event) in events.iter().enumerate().take(streaming_end + 1) {
            let Event::Return { sensor_ts } = *event else { continue };
            let serials = |only: Option<i64>| {
                events[..position]
                    .iter()
                    .filter_map(move |earlier| match *earlier {
                        Event::Draw {
                            sensor_ts: drawn,
                            serial,
                        } if only.is_none_or(|only| only == drawn) => Some(serial),
                        _ => None,
                    })
                    .max()
            };
            if let Some(last_read) = serials(Some(sensor_ts)) {
                assert!(
                    serials(None).is_some_and(|latest| latest > last_read),
                    "кадр {sensor_ts} возвращён, хотя позднее сабмита {last_read} других сабмитов не было"
                );
            }
        }
    }

    /// Кадры, прочитанные GPU во время показа, вернулись с НАСТОЯЩИМ fence (экспорт сабмита, который их
    /// читал), а не «просто так».
    pub fn assert_streaming_returns_carry_real_fences(&self, streamed: usize) {
        let drawn: Vec<i64> = self
            .log
            .snapshot()
            .iter()
            .filter_map(|event| match event {
                Event::Draw { sensor_ts, .. } => Some(*sensor_ts),
                Event::Return { .. } => None,
            })
            .collect();
        let returned = self.book.returned.lock().unwrap_or_else(PoisonError::into_inner);
        let with_real_fence = returned
            .iter()
            .take(streamed)
            .filter(|buffer| drawn.contains(&buffer.sensor_ts))
            .filter(|buffer| !buffer.fences.is_empty())
            .count();
        let drawn_and_returned = returned
            .iter()
            .take(streamed)
            .filter(|buffer| drawn.contains(&buffer.sensor_ts))
            .count();
        assert!(
            drawn_and_returned > 0,
            "ни один прочитанный кадр не вернулся во время показа"
        );
        assert_eq!(
            with_real_fence, drawn_and_returned,
            "каждый прочитанный кадр вернулся с fence своего сабмита"
        );
    }

    /// Каждый fence, с которым буфер вернулся производителю, срабатывает (GPU закончил с буфером).
    pub fn assert_all_fences_signal(&self) {
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
pub enum ReturnRules {
    /// Все правила ISP: кадр читается одним сабмитом и возвращается его fence, на руках ≤ 1 кадра.
    Full,
    /// Правила отладочной мозаики: возврат только после более позднего сабмита.
    DebugMosaic,
    /// Только «не читается после возврата» и сигнал fence-ов (surface отсоединялась посреди показа).
    NoReadAfterReturn,
}
