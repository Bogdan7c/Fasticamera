//! Сборка кадра снимка для тестов.

use std::sync::Arc;

use camera_contract::capabilities::{LensShading, SensorLevels, VendorProcessingFlags};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::FrameChoice;
use still_contract::lens_shading::{AppliedLensShading, LensShadingNotAppliedReason};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId, StillOutputs};
use still_contract::shot::{
    AsShotNeutral, DigitalGainEv, OwnedPixels, RawSamples16, Rgba8Image, StillFrameFacts, StillRotation,
};

/// Размер основной картинки в тестах.
pub const PRIMARY_SIZE: PixelSize = PixelSize::new(8, 6);
/// Размер сырого кадра в тестах.
pub const RAW_SIZE: PixelSize = PixelSize::new(16, 12);

pub fn facts_for(id: u64, primary: PrimaryFormat, raw: RawOutput) -> Box<StillFrameFacts> {
    let buffer_size = RAW_SIZE;
    let passport = StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw10,
        buffer_size,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: buffer_size,
        },
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(buffer_size),
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
        // Калибровка образца основной камеры: настоящий писатель DNG без ColorMatrix1 файл не пишет.
        color_calibration: fixture_calibration(),
    };
    let timestamp = SensorTimestamp::from_nanos(1_000_000 + i64::try_from(id).unwrap_or(0));
    Box::new(StillFrameFacts {
        id: StillId(id),
        outputs: StillOutputs { primary, raw },
        stream: Arc::new(passport),
        sensor_timestamp: timestamp,
        capture: FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(7),
                sensor_timestamp: Some(timestamp),
                exposure_time: None,
                sensitivity_iso: Some(100),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints::default(),
        },
        choice: FrameChoice::ShownAtPress {
            sensor_timestamp: timestamp,
            shown_at: MonotonicNanos::from_nanos(5),
        },
        crop: PixelRect {
            left: 0,
            top: 0,
            size: PRIMARY_SIZE,
        },
        rotation: StillRotation::Clockwise90,
        neutral: AsShotNeutral([0.5, 1.0, 0.6]),
        digital_gain: DigitalGainEv(0.0),
        lens_shading: AppliedLensShading::NotApplied(LensShadingNotAppliedReason::NotCalibrated),
    })
}

/// Цветовая калибровка образца основной камеры OnePlus 11 (`camera_contract::fixtures`).
fn fixture_calibration() -> Vec<camera_contract::capabilities::ColorCalibrationSet> {
    match camera_contract::fixtures::main_wide_camera().capabilities.raw {
        camera_contract::capabilities::RawSupport::Supported(info) => info.color_calibration,
        other => panic!("у образца основной камеры нет RAW: {other:?}"),
    }
}

/// Сырые отсчёты для DNG.
pub fn raw_for() -> RawSamples16 {
    let bytes = vec![0_u8; usize::try_from(RAW_SIZE.pixel_count() * 2).expect("влезает")];
    RawSamples16::new(RAW_SIZE, Arc::new(OwnedPixels(bytes))).expect("размер совпал")
}

/// Картинка для JPEG: байты заполнены `fill`, чтобы тест видел, что ушло именно ЭТО в сжатие.
pub fn readback_for(fill: u8) -> Rgba8Image {
    let bytes = vec![fill; usize::try_from(PRIMARY_SIZE.pixel_count() * 4).expect("влезает")];
    Rgba8Image::new(PRIMARY_SIZE, Arc::new(OwnedPixels(bytes))).expect("размер совпал")
}
