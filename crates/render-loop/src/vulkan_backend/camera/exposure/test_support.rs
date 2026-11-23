//! Общий стенд тестов AE без GPU: потоки и кадры паспорта, синтетическая статистика, `Harness` с runner.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use std::time::Duration;

use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, FrameSequence, MissingCaptureMeta};
use camera_contract::geometry::{PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{
    MeteringRegion, PointOfInterest, PointRequestId, RequestOrigin, SensorPoint, ThreeAResultState,
};
use isp_model::statistics::{AwbTile, TileGrid, quantize_sample};

use super::*;

pub(super) const CELLS: u32 = 4096;
pub(in crate::vulkan_backend::camera) const FRAME_NANOS: i64 = 16_666_667;
/// Цель тона в тестах: то же число, что у боевого рендера.
pub(in crate::vulkan_backend::camera) const TARGET_GRAY: f32 = 0.075;
/// Яркость, которую точно представляет 12-битная статистика: 77/4095.
pub(super) const DARK_VALUE: f32 = 77.0 / 4095.0;
/// 2048/4095 ≈ 0,5.
pub(super) const BRIGHT_VALUE: f32 = 2048.0 / 4095.0;
/// Веса яркости суммой 1: для серого `v` яркость равна `v`.
pub(in crate::vulkan_backend::camera) const LUMINANCE: LuminanceWeights = LuminanceWeights::new(0.25, 0.5, 0.25);
pub(in crate::vulkan_backend::camera) const BUFFER: PixelSize = PixelSize::new(64, 48);

pub(in crate::vulkan_backend::camera) fn stream_of(readout: ReadoutAspect, valid_height: u32) -> Arc<StreamPassport> {
    Arc::new(StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: BUFFER,
        readout,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(BUFFER.width, valid_height),
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(BUFFER),
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
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: Vec::new(),
    })
}

/// Новый поток 4:3 (каждый вызов — новый `Arc`, то есть новая сессия камеры).
pub(in crate::vulkan_backend::camera) fn new_stream() -> Arc<StreamPassport> {
    stream_of(ReadoutAspect::FourByThree, BUFFER.height)
}

/// Выдержка и ISO результата кадра.
#[derive(Debug, Clone, Copy)]
pub(in crate::vulkan_backend::camera) struct Exposure {
    pub(in crate::vulkan_backend::camera) time: Duration,
    pub(in crate::vulkan_backend::camera) iso: u32,
}

/// 10 мс × ISO 100 = 1 с·ISO: удобная единица.
pub(in crate::vulkan_backend::camera) const UNIT_EXPOSURE: Exposure = Exposure {
    time: Duration::from_millis(10),
    iso: 100,
};

pub(in crate::vulkan_backend::camera) fn exposure_product(seconds_iso: f64) -> Exposure {
    Exposure {
        time: Duration::from_secs_f64(seconds_iso / 100.0),
        iso: 100,
    }
}

pub(in crate::vulkan_backend::camera) fn frame_at(
    stream: &Arc<StreamPassport>,
    index: i64,
    exposure: Option<Exposure>,
) -> RawFramePassport {
    frame_with_keys(
        stream,
        index,
        exposure.map(|exposure| exposure.time),
        exposure.map(|exposure| exposure.iso),
    )
}

pub(in crate::vulkan_backend::camera) fn frame_with_keys(
    stream: &Arc<StreamPassport>,
    index: i64,
    exposure_time: Option<Duration>,
    sensitivity_iso: Option<u32>,
) -> RawFramePassport {
    let sensor_timestamp = SensorTimestamp::from_nanos(1_000_000_000 + index * FRAME_NANOS);
    RawFramePassport {
        stream: Arc::clone(stream),
        sequence: FrameSequence(u64::try_from(index).expect("номер кадра")),
        sensor_timestamp,
        capture: FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(index),
                sensor_timestamp: Some(sensor_timestamp),
                exposure_time,
                sensitivity_iso,
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints::default(),
        }),
    }
}

/// Кадр с заданным происхождением запроса и тем, что HAL сообщил о вспышке.
pub(in crate::vulkan_backend::camera) fn flash_frame_at(
    stream: &Arc<StreamPassport>,
    index: i64,
    exposure: Option<Exposure>,
    origin: RequestOrigin,
    flash: camera_contract::flash::FlashResult,
) -> RawFramePassport {
    let mut passport = frame_at(stream, index, exposure);
    if let FrameCaptureMeta::Paired(details) = &mut passport.capture {
        details.result.three_a.origin = origin;
        details.result.three_a.flash = flash;
    }
    passport
}

pub(in crate::vulkan_backend::camera) fn missing_frame(stream: &Arc<StreamPassport>, index: i64) -> RawFramePassport {
    RawFramePassport {
        capture: FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::PairingTimedOut,
        },
        ..frame_at(stream, index, None)
    }
}

pub(super) fn identity_of(passport: &RawFramePassport) -> AwbFrameIdentity {
    AwbFrameIdentity {
        sequence: passport.sequence.0,
        sensor_timestamp_nanos: passport.sensor_timestamp.as_nanos(),
        frame_number: None,
    }
}

pub(super) fn facts_of(passport: &RawFramePassport) -> ExposureFrameFacts {
    ExposureFrameFacts {
        product: frame_exposure_product(passport),
        luminance: LUMINANCE,
        crop: passport.stream.valid_area,
        light: frame_light(passport),
    }
}

// ---- синтетическая статистика ----

pub(super) fn grid() -> TileGrid {
    TileGrid { columns: 32, rows: 24 }
}

pub(super) fn gray_tile(value: f32) -> AwbTile {
    let q = quantize_sample(value);
    AwbTile {
        sum_r: q * CELLS,
        sum_g: 2 * q * CELLS,
        sum_b: q * CELLS,
        used_cells: CELLS,
        saturated_cells: 0,
    }
}

pub(super) fn uniform_scene(value: f32) -> AwbStatistics {
    AwbStatistics::from_tiles(grid(), vec![gray_tile(value); 768]).expect("768 плиток")
}

/// Левая половина кадра яркая, правая тёмная.
pub(super) fn bright_window_left_scene() -> AwbStatistics {
    let tiles = (0..768)
        .map(|index| gray_tile(if index % 32 < 16 { BRIGHT_VALUE } else { DARK_VALUE }))
        .collect();
    AwbStatistics::from_tiles(grid(), tiles).expect("768 плиток")
}

// ---- стенд ----

pub(super) struct Harness {
    pub(super) runner: ExposureRunner,
    pub(super) directives: DirectiveSlot<ExposureDirective>,
    pub(super) proposals: ExposureProposalChannel,
    pub(super) stream: Arc<StreamPassport>,
    pub(super) next_index: i64,
}

impl Harness {
    pub(super) fn with_config(config: AutoExposureConfig) -> Self {
        let directives = DirectiveSlot::new();
        let proposals = ExposureProposalChannel::new();
        let runner = ExposureRunner::new(
            AutoExposureParts {
                config,
                directives: directives.clone(),
                proposals: proposals.clone(),
            },
            TARGET_GRAY,
        )
        .expect("AE собирается");
        Self {
            runner,
            directives,
            proposals,
            stream: new_stream(),
            next_index: 0,
        }
    }

    pub(super) fn new() -> Self {
        Self::with_config(AutoExposureConfig::default())
    }

    /// Кадр: план, затем (как после fence слота) приём статистики этого же кадра.
    pub(super) fn step(&mut self, exposure: Option<Exposure>, statistics: &AwbStatistics) -> ExposureFramePlan {
        let passport = frame_at(&self.stream, self.next_index, exposure);
        self.next_index += 1;
        let plan = self.runner.plan_frame(&passport);
        self.runner
            .ingest_statistics(plan.stream, identity_of(&passport), &facts_of(&passport), statistics);
        plan
    }

    pub(super) fn steps(
        &mut self,
        count: usize,
        exposure: Option<Exposure>,
        statistics: &AwbStatistics,
    ) -> ExposureFramePlan {
        let mut last = None;
        for _ in 0..count {
            last = Some(self.step(exposure, statistics));
        }
        last.expect("хотя бы один кадр")
    }

    pub(super) fn published(&self) -> u64 {
        self.runner.diagnostics().counters.proposals_published
    }

    pub(super) fn latest_target(&self) -> f64 {
        self.proposals
            .latest()
            .expect("предложение есть")
            .1
            .target
            .seconds_iso()
    }
}

/// Произведение, которое замер обязан потребовать для яркости `value` при кадре с 1 с·ISO.
pub(super) fn required_product(value: f32) -> f64 {
    f64::from(TARGET_GRAY) / f64::from(value)
}

pub(super) fn assert_close_ev(actual: f64, expected: f64, tolerance_ev: f64, what: &str) {
    let difference = (actual / expected).log2().abs();
    assert!(
        difference <= tolerance_ev,
        "{what}: {actual} против {expected}, разница {difference} EV"
    );
}

pub(super) fn point_directive(x: f32, y: f32, request: u64) -> ExposureDirective {
    let region = MeteringRegion::new(SensorPoint::new(x, y).expect("точка"), 0.1).expect("регион");
    ExposureDirective {
        metering: PointOfInterest::Point {
            region,
            request: PointRequestId(request),
        },
        ..ExposureDirective::DEFAULT
    }
}
