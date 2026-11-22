//! Помощники тестов движка снимков: настоящий `RawFrameStream` с кольцом, поддельный GPU с управляемым
//! поведением, приёмник событий, планы и каталог зума. Без GPU и без Android.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "помощники тестов: падение — способ сообщить о провале"
)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use camera_contract::capabilities::{LensShading, RawSupport, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::flash::{FlashResult, FlashShotId};
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::lens_calibration::{OpticalAxis, UncalibratedReason};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use camera_contract::viewfinder_crop::{CropAspect, ZoomFactor};
use camera_contract::zoom::{
    LensGroup, LensZoomPoint, PreviewMirror, ZoomCatalog, ZoomFraming, ZoomRange, ZoomRatio, ZoomStop, ZoomStopKind,
};
use gpu_vulkan::sync::GpuWaitFence;
use raw_frame_stream::{OfferedFrame, ProducerFrame, RawFrameStream, RawFrameStreamConfig, ReleaseFences, StreamEpoch};
use shell_contract::clock::MonotonicNanos;
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::{NativeSurface, PlatformWindow, SurfaceGeneration};
use still_contract::develop::DevelopEvent;
use still_contract::plan::{FrameSelection, PrimaryFormat, RawOutput, ShutterPress, StillId, StillOutputs, StillPlan};

use super::engine::{GpuMaker, StillClock, StillEngine};
use super::feed::{FrameStillParams, ShownNow, StillFeed, StillFeedRecorder};
use crate::config::{StillDevelopConfig, ViewfinderToneConfig};
use crate::still::StillEventSink;

pub const MAIN: &str = "main";
pub const FRONT: &str = "front";
pub const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
/// Шаг меток сенсора между кадрами (60 к/с).
pub const SENSOR_STEP: i64 = 16_666_667;
pub const FIRST_TS: i64 = 1_000_000_000;

pub fn stamp(nanos: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(nanos)
}

pub fn ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(millis * 1_000_000)
}

// ---- каталог и план ----

fn lens(camera: &str, role: LensRole) -> LensZoomPoint {
    LensZoomPoint {
        role,
        camera: camera_id(camera),
        native_ratio: ZoomRatio::new(1.0).expect("коэффициент"),
        equivalent_focal_mm: 24.0,
        optical_axis: OpticalAxis::Uncalibrated {
            reason: UncalibratedReason::NotReported,
        },
        active_array: FRAME_SIZE,
        preview_mirror: PreviewMirror::None,
        max_digital_zoom: ZoomFactor::new(10.0).expect("зум"),
    }
}

fn group(facing: LensFacing, camera: &str, role: LensRole) -> LensGroup {
    let lens = lens(camera, role);
    LensGroup {
        facing,
        stops: vec![ZoomStop {
            ratio: lens.native_ratio,
            kind: ZoomStopKind::Lens(role),
        }],
        lenses: vec![lens],
        range: ZoomRange {
            min: ZoomRatio::new(1.0).expect("коэффициент"),
            max: ZoomRatio::new(10.0).expect("коэффициент"),
        },
    }
}

/// Каталог: задняя `main`, фронтальная `front`.
pub fn catalog() -> Arc<ZoomCatalog> {
    Arc::new(ZoomCatalog {
        back: Some(group(LensFacing::Back, MAIN, LensRole::Wide)),
        front: Some(group(LensFacing::Front, FRONT, LensRole::Front)),
    })
}

pub fn framing(aspect: CropAspect, ratio: f32) -> ZoomFraming {
    ZoomFraming {
        aspect,
        facing: LensFacing::Back,
        ratio: ZoomRatio::new(ratio).expect("коэффициент"),
        catalog: catalog(),
    }
}

/// План снимка: нажатие в `press_ms` (MONOTONIC), весь кадр 1×, портрет, JPEG без DNG.
pub fn plan(id: u64, press_ms: i64) -> StillPlan {
    StillPlan {
        id: StillId(id),
        press: ShutterPress { at: ms(press_ms) },
        selection: FrameSelection::ShownAtPress,
        framing: framing(CropAspect::SensorNative, 1.0),
        orientation: DeviceOrientation::Portrait,
        outputs: StillOutputs {
            primary: PrimaryFormat::Jpeg,
            raw: RawOutput::Skip,
        },
    }
}

pub fn plan_with_dng(id: u64, press_ms: i64) -> StillPlan {
    StillPlan {
        outputs: StillOutputs {
            primary: PrimaryFormat::Jpeg,
            raw: RawOutput::Dng,
        },
        ..plan(id, press_ms)
    }
}

// ---- поток кадров ----

pub fn stream_passport(camera: &str) -> StreamPassport {
    let calibration = match main_wide_camera().capabilities.raw {
        RawSupport::Supported(info) => info.color_calibration,
        RawSupport::NotSupported => panic!("в образце основной камеры есть RAW"),
    };
    StreamPassport {
        camera: camera_id(camera),
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
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: calibration,
    }
}

#[derive(Default)]
pub struct Returned(pub Mutex<Vec<i64>>);

impl Returned {
    pub fn snapshot(&self) -> Vec<i64> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

struct FakeFrame {
    sensor_ts: i64,
    storage: Box<u8>,
    returned: Arc<Returned>,
}

// SAFETY: указатель из `hardware_buffer` указывает на `storage`, живой до `return_to_producer`; настоящий буфер
// тестам не нужен — движок снимков с ним ничего не делает (импорт — забота настоящего GPU).
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

/// Настоящий поток кадров с кольцом; кадры приходят вместе с результатами съёмки.
pub struct StreamRig {
    pub stream: RawFrameStream,
    pub passport: Arc<StreamPassport>,
    pub epoch: StreamEpoch,
    pub returned: Arc<Returned>,
    pub next_ts: i64,
    next_frame_number: i64,
}

impl StreamRig {
    pub fn new(config: RawFrameStreamConfig, camera: &str) -> Self {
        let stream = RawFrameStream::new(config);
        let passport = Arc::new(stream_passport(camera));
        let epoch = stream.producer().begin_stream(Arc::clone(&passport)).epoch;
        Self {
            stream,
            passport,
            epoch,
            returned: Arc::default(),
            next_ts: FIRST_TS,
            next_frame_number: 1,
        }
    }

    pub fn with_ring(camera: &str) -> Self {
        Self::new(
            RawFrameStreamConfig {
                viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
                ..RawFrameStreamConfig::default()
            },
            camera,
        )
    }

    /// Следующий кадр с результатом съёмки и происхождением `origin`; возвращает метку.
    pub fn offer_with_origin(&mut self, origin: RequestOrigin, with_metadata: bool) -> SensorTimestamp {
        self.offer_detailed(origin, with_metadata, None, FlashResult::UNREPORTED)
    }

    /// Кадр запроса со вспышкой снимка `shot`: выдержка × ISO = 1 с·ISO (10 мс × 100), HAL сообщил `flash`.
    pub fn offer_flash_still(&mut self, shot: u64, flash: FlashResult) -> SensorTimestamp {
        self.offer_detailed(
            RequestOrigin::FlashStill {
                shot: FlashShotId(shot),
            },
            true,
            Some(Duration::from_millis(10)),
            flash,
        )
    }

    /// Кадр с полным набором того, что движок снимков читает из результата.
    pub fn offer_detailed(
        &mut self,
        origin: RequestOrigin,
        with_metadata: bool,
        exposure_time: Option<Duration>,
        flash: FlashResult,
    ) -> SensorTimestamp {
        let ts = self.next_ts;
        self.next_ts += SENSOR_STEP;
        let number = self.next_frame_number;
        self.next_frame_number += 1;
        self.stream.producer().offer_frame(
            self.epoch,
            OfferedFrame {
                sensor_timestamp: stamp(ts),
                acquire_fence: GpuWaitFence::already_signaled(),
                frame: Box::new(FakeFrame {
                    sensor_ts: ts,
                    storage: Box::new(0),
                    returned: Arc::clone(&self.returned),
                }),
            },
            Instant::now(),
        );
        if with_metadata {
            self.stream.producer().offer_capture_result(
                self.epoch,
                FrameCaptureDetails {
                    result: CaptureResultMeta {
                        frame_number: FrameNumber(number),
                        sensor_timestamp: Some(stamp(ts)),
                        exposure_time,
                        sensitivity_iso: Some(100),
                        reported_frame_duration: None,
                        rolling_shutter_skew: None,
                        three_a: ThreeAResultState {
                            flash,
                            ..ThreeAResultState::unreported(origin)
                        },
                    },
                    color: CaptureColorHints::default(),
                },
                Instant::now(),
            );
        }
        stamp(ts)
    }

    pub fn offer(&mut self) -> SensorTimestamp {
        self.offer_with_origin(RequestOrigin::Repeating, true)
    }

    pub fn offer_many(&mut self, count: usize) -> Vec<SensorTimestamp> {
        (0..count).map(|_| self.offer()).collect()
    }

    pub fn params_for(&self) -> FrameStillParams {
        FrameStillParams {
            stream: Arc::clone(&self.passport),
            white: isp_model::color::chromaticity::Chromaticity { x: 0.3127, y: 0.3290 },
            gain: isp_model::exposure::ExposureGain::UNITY,
        }
    }

    /// Лента видоискателя: каждый кадр из `frames` показан под номером `first_present + i` с параметрами кадра.
    pub fn feed_for(&self, frames: &[SensorTimestamp], first_present: u32) -> StillFeed {
        let mut recorder = StillFeedRecorder::default();
        for (index, frame) in frames.iter().enumerate() {
            recorder.note_frame_params(*frame, self.params_for());
            let present_id = first_present + u32::try_from(index).unwrap_or(0);
            recorder.note_present(present_id, ShownNow::NewLiveFrame(*frame));
        }
        recorder.take()
    }
}

// ---- приёмник событий ----

#[derive(Default)]
pub struct Sink(pub Mutex<Vec<DevelopEvent>>);

impl StillEventSink for Sink {
    fn deliver(&self, event: DevelopEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

impl Sink {
    pub fn take(&self) -> Vec<DevelopEvent> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Сколько итоговых событий пришло (по id).
    pub fn finals_for(&self, id: StillId) -> usize {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|event| event.id() == id && event.is_final())
            .count()
    }
}

// ---- окно ----

struct FakeWindow;

impl PlatformWindow for FakeWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

pub fn fake_encoder_surface() -> NativeSurface {
    NativeSurface::new(SurfaceGeneration::new(1), Box::new(FakeWindow))
}

// ---- часы ----

#[derive(Clone)]
pub struct ManualClock {
    origin: Instant,
    offset: Arc<Mutex<Duration>>,
}

impl ManualClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            offset: Arc::default(),
        }
    }

    pub fn advance(&self, by: Duration) {
        *self.offset.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }
}

impl StillClock for ManualClock {
    fn now(&self) -> Instant {
        self.origin + *self.offset.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub use super::fake_gpu::*;

// ---- сборка движка ----

pub struct EngineRig {
    pub engine: StillEngine<FakeGpu>,
    pub stream: StreamRig,
    pub gpu: SharedState,
    pub clock: ManualClock,
    pub made_gpus: Arc<Mutex<u32>>,
}

pub fn engine_rig(config: StillDevelopConfig, stream: StreamRig) -> EngineRig {
    let gpu = gpu_state();
    let clock = ManualClock::new();
    let made: Arc<Mutex<u32>> = Arc::default();
    let maker_state = Arc::clone(&gpu);
    let maker_count = Arc::clone(&made);
    let maker: GpuMaker<FakeGpu> = Box::new(move |_stream| {
        *maker_count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        Ok(FakeGpu {
            state: Arc::clone(&maker_state),
            served: FRAME_SIZE,
        })
    });
    let engine = StillEngine::new(
        config,
        ViewfinderToneConfig::default(),
        crate::vulkan_backend::camera::exposure::ResidualGainLimits::of(&crate::config::AutoExposureConfig::default()),
        stream.stream.snapshot_ring(),
        maker,
        Box::new(clock.clone()),
    );
    EngineRig {
        engine,
        stream,
        gpu,
        clock,
        made_gpus: made,
    }
}

impl EngineRig {
    /// Начинает снимок и возвращает приёмник его событий.
    pub fn begin(&mut self, plan: StillPlan) -> Arc<Sink> {
        let sink = Arc::new(Sink::default());
        let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
        let outcome = self
            .engine
            .apply_command(crate::still::StillCommand::Begin(crate::still::StillBegin {
                plan,
                events,
            }));
        assert!(matches!(outcome, crate::still::StillCommandOutcome::Accepted));
        sink
    }

    /// Несколько шагов движка с продвижением часов.
    pub fn run(&mut self, steps: usize) {
        for _ in 0..steps {
            self.clock.advance(Duration::from_millis(16));
            self.engine.advance();
        }
    }
}

pub fn queue_of(events: Vec<DevelopEvent>) -> VecDeque<DevelopEvent> {
    events.into()
}
