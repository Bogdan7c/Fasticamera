//! Поддельный производитель, журнал возвратов и сборка стенда для тестов.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::Read;
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::sync_fence::{GpuReleaseFence, GpuWaitFence};

use crate::config::RawFrameStreamConfig;
use crate::producer_frame::{ProducerFrame, ReleaseFences};
use crate::stream::{OfferedFrame, RawFrameProducer, RawFrameStream, StreamEpoch, StreamStart};

/// Что вернули производителю: сколько fence-ов приехало и сами концы fence-ов для проверки сигнала.
struct ReturnedBuffer {
    fences: Vec<UnixStream>,
    signaled: Vec<bool>,
}

/// Журнал возвратов. Буфер опознаётся по метке сенсора в наносекундах (она же его «id»).
#[derive(Default)]
pub(super) struct ReturnLog {
    returned: Mutex<BTreeMap<i64, ReturnedBuffer>>,
}

impl ReturnLog {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<i64, ReturnedBuffer>> {
        self.returned.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn is_returned(&self, buffer: i64) -> bool {
        self.lock().contains_key(&buffer)
    }

    pub(super) fn returned_count(&self) -> usize {
        self.lock().len()
    }

    /// Сколько fence-ов приехало вместе с буфером (None — буфер не возвращали).
    pub(super) fn fence_count(&self, buffer: i64) -> Option<usize> {
        self.lock().get(&buffer).map(|returned| returned.fences.len())
    }

    /// Модель пула камеры: буфер свободен, когда он возвращён и все его fence-ы просигналены.
    pub(super) fn is_free(&self, buffer: i64) -> bool {
        let mut returned = self.lock();
        let Some(entry) = returned.get_mut(&buffer) else {
            return false;
        };
        for (fence, signaled) in entry.fences.iter_mut().zip(entry.signaled.iter_mut()) {
            if !*signaled {
                let mut byte = [0_u8; 1];
                *signaled = matches!(fence.read(&mut byte), Ok(1));
            }
        }
        entry.signaled.iter().all(|signaled| *signaled)
    }
}

struct FakeFrame {
    buffer: i64,
    storage: Box<u8>,
    log: Arc<ReturnLog>,
}

// SAFETY: указатель в `hardware_buffer` указывает на `storage`, который живёт, пока жив
// сам `FakeFrame` (до `return_to_producer`), как требует контракт трейта.
unsafe impl ProducerFrame for FakeFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        let pointer = NonNull::from(&*self.storage).cast::<c_void>();
        // SAFETY: см. выше — `storage` живой на время заимствования `&self`.
        unsafe { BorrowedHardwareBuffer::from_raw(pointer) }
    }

    fn return_to_producer(self: Box<Self>, release: ReleaseFences) {
        let fences: Vec<UnixStream> = release
            .into_vec()
            .into_iter()
            .filter_map(GpuReleaseFence::into_owned_fd)
            .map(UnixStream::from)
            .collect();
        for fence in &fences {
            fence.set_nonblocking(true).expect("неблокирующий fence");
        }
        let signaled = vec![false; fences.len()];
        self.log.lock().insert(self.buffer, ReturnedBuffer { fences, signaled });
    }
}

/// Стенд: поток, ручка производителя, журнал и начатая эпоха.
pub(super) struct Rig {
    pub(super) stream: RawFrameStream,
    pub(super) producer: RawFrameProducer,
    pub(super) log: Arc<ReturnLog>,
    pub(super) epoch: StreamEpoch,
    pub(super) start: StreamStart,
    pub(super) t0: Instant,
    start_passport: Arc<StreamPassport>,
}

impl Rig {
    pub(super) fn new(config: RawFrameStreamConfig) -> Self {
        Self::with_passport(config, passport(RawFormat::Raw10, PixelSize::new(4096, 3072)))
    }

    pub(super) fn with_passport(config: RawFrameStreamConfig, passport: StreamPassport) -> Self {
        let stream = RawFrameStream::new(config);
        let producer = stream.producer();
        let passport = Arc::new(passport);
        let start = producer.begin_stream(Arc::clone(&passport));
        Self {
            epoch: start.epoch,
            start,
            stream,
            producer,
            log: Arc::new(ReturnLog::default()),
            t0: Instant::now(),
            start_passport: passport,
        }
    }

    /// Новая сессия камеры (новая эпоха) на том же потоке; пул пересчитывается.
    pub(super) fn restart(&mut self) {
        let passport = self.start_passport.clone();
        self.start = self.producer.begin_stream(passport);
        self.epoch = self.start.epoch;
    }

    pub(super) fn at(&self, millis: u64) -> Instant {
        self.t0 + Duration::from_millis(millis)
    }

    /// Кадр с буфером, «опознаваемым» по метке `timestamp`.
    pub(super) fn frame(&self, timestamp: i64) -> OfferedFrame {
        frame_for(&self.log, timestamp)
    }

    pub(super) fn offer_frame(&self, timestamp: i64, millis: u64) -> crate::stream::OfferOutcome {
        self.producer
            .offer_frame(self.epoch, self.frame(timestamp), self.at(millis))
    }

    pub(super) fn offer_result(&self, timestamp: i64, millis: u64) -> crate::stream::ResultOutcome {
        self.producer
            .offer_capture_result(self.epoch, result_for(timestamp), self.at(millis))
    }
}

pub(super) fn frame_for(log: &Arc<ReturnLog>, timestamp: i64) -> OfferedFrame {
    OfferedFrame {
        sensor_timestamp: SensorTimestamp::from_nanos(timestamp),
        acquire_fence: GpuWaitFence::already_signaled(),
        frame: Box::new(FakeFrame {
            buffer: timestamp,
            storage: Box::new(0),
            log: Arc::clone(log),
        }),
    }
}

/// Номер кадра в результате выводится из метки, чтобы тест видел «чьи» это метаданные.
pub(super) fn frame_number_for(timestamp: i64) -> FrameNumber {
    FrameNumber(timestamp / 1000 + 7)
}

pub(super) fn result_for(timestamp: i64) -> FrameCaptureDetails {
    FrameCaptureDetails {
        result: CaptureResultMeta {
            frame_number: frame_number_for(timestamp),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(timestamp)),
            exposure_time: None,
            sensitivity_iso: Some(100),
            reported_frame_duration: None,
            rolling_shutter_skew: None,
        },
        color: CaptureColorHints::default(),
    }
}

pub(super) fn passport(raw_format: RawFormat, buffer_size: PixelSize) -> StreamPassport {
    StreamPassport {
        camera: camera_id("2"),
        raw_format,
        buffer_size,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: buffer_size,
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

/// Пара сокетов как fence: первая половина идёт как release-fence, вторая сигналит записью байта.
pub(super) fn unsignaled_fence() -> (GpuReleaseFence, UnixStream) {
    let (fence_end, signal_end) = UnixStream::pair().expect("пара сокетов");
    (GpuReleaseFence::from_sync_fd(fence_end.into()), signal_end)
}

pub(super) fn signal(signal_end: &mut UnixStream) {
    use std::io::Write;
    signal_end.write_all(&[1]).expect("сигнал fence");
}
