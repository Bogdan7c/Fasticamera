//! Фикстура снятого кадра OnePlus 11 (основная камера, BGGR, RAW10 → u16) для тестов DNG и Exif.
//!
//! Уровни, раскладка CFA и камера берутся из `camera_contract::fixtures`; матрицы цвета — НАСТОЯЩИЕ
//! числа паспорта эталонного кадра `204802-f42af9` (лампа), чтобы тесты сверяли не единицы, а различимые значения.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "тестовая фикстура"
)]

use std::sync::Arc;
use std::time::Duration;

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Matrix3x3, Rational, RawSupport, ReferenceIlluminant, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::main_wide_camera;
use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureDetails, StreamPassport};
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::raw_format::{MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use photo_output::exif::{CameraDescription, CaptureMoment, LensDescription};
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::FrameChoice;
use still_contract::lens_shading::{AppliedLensShading, LensShadingNotAppliedReason};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId, StillOutputs};
use still_contract::shot::{AsShotNeutral, DigitalGainEv, OwnedPixels, RawSamples16, StillFrameFacts, StillRotation};

/// Матрица 3×3 из десятичных чисел паспорта: знаменатель 10000 (числа паспорта — до 4 знаков).
pub fn matrix_from(values: [f64; 9]) -> Matrix3x3 {
    Matrix3x3(values.map(|value| Rational {
        numerator: (value * 10_000.0).round() as i32,
        denominator: 10_000,
    }))
}

/// Два набора калибровки основной камеры: D65 и лампа A (`2_lamp.passport.json`).
pub fn main_camera_calibration() -> Vec<ColorCalibrationSet> {
    vec![
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::D65,
            color_matrix: matrix_from([
                0.9848, -0.3684, -0.1011, -0.2682, 1.1295, 0.1152, 0.0588, 0.0403, 0.5348,
            ]),
            forward_matrix: matrix_from([0.4375, 0.3828, 0.1406, 0.2188, 0.7188, 0.0625, 0.0156, 0.0938, 0.7109]),
            calibration_transform: matrix_from([1.0078, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0078]),
        },
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::StandardA,
            color_matrix: matrix_from([1.9084, -1.3277, 0.1113, -0.2221, 1.1645, 0.2236, 0.141, -0.0884, 0.8508]),
            // Матрица второго иллюминанта нарочно отличается от первой (в паспорте эталона они равны):
            // перепутанные ForwardMatrix1/2 должны ловиться тестом. Строки дают белый D50 (0,9642; 1; 0,8249),
            // как требует `dng_validate`.
            forward_matrix: matrix_from([0.45, 0.37, 0.1442, 0.2, 0.73, 0.07, 0.02, 0.1, 0.7049]),
            calibration_transform: matrix_from([0.9922, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.9766]),
        },
    ]
}

/// Паспорт потока основной камеры; `valid_area` задаёт тест (размер отсчётов = её размер).
pub fn main_camera_stream(valid_area: PixelRect) -> StreamPassport {
    let descriptor = main_wide_camera();
    let RawSupport::Supported(raw_info) = &descriptor.capabilities.raw else {
        panic!("у основной камеры фикстуры есть RAW");
    };
    StreamPassport {
        camera: descriptor.id.clone(),
        raw_format: RawFormat::Raw10,
        buffer_size: PixelSize::new(4096, 3072),
        readout: ReadoutAspect::FourByThree,
        valid_area,
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(PixelSize::new(4096, 3072)),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: raw_info.cfa,
        },
        static_levels: raw_info.levels,
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: descriptor.capabilities.sensor_orientation,
        frame_rate: TargetFrameRate::new(30).unwrap(),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: main_camera_calibration(),
    }
}

/// Факты кадра: выдержка 22,450523 мс, ISO 249, нейтраль и подтяжка — отличимые числа.
pub fn still_facts(stream: StreamPassport, crop: PixelRect) -> StillFrameFacts {
    StillFrameFacts {
        id: StillId(7),
        outputs: StillOutputs {
            primary: PrimaryFormat::Heif,
            raw: RawOutput::Dng,
        },
        stream: Arc::new(stream),
        sensor_timestamp: SensorTimestamp::from_nanos(1_445_238_949_828_037),
        capture: FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(11),
                sensor_timestamp: Some(SensorTimestamp::from_nanos(1_445_238_949_828_037)),
                exposure_time: Some(Duration::from_nanos(22_450_523)),
                sensitivity_iso: Some(249),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints::default(),
        },
        choice: FrameChoice::ShownAtPress {
            sensor_timestamp: SensorTimestamp::from_nanos(1_445_238_949_828_037),
            shown_at: MonotonicNanos::from_nanos(5),
        },
        crop,
        rotation: StillRotation::Clockwise90,
        neutral: AsShotNeutral([0.515625, 1.0, 0.4990234375]),
        digital_gain: DigitalGainEv(0.25),
        lens_shading: AppliedLensShading::NotApplied(LensShadingNotAppliedReason::NotCalibrated),
    }
}

/// Отсчёты размера `size`: значение зависит от позиции (разные у соседей), чтобы перепутанные строки
/// или полосы были видны.
pub fn synthetic_raw(size: PixelSize) -> RawSamples16 {
    let mut bytes = Vec::with_capacity((size.pixel_count() * 2) as usize);
    for y in 0..size.height {
        for x in 0..size.width {
            let sample = ((y * 131 + x * 7 + 64) % 1024) as u16;
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    RawSamples16::new(size, Arc::new(OwnedPixels(bytes))).unwrap()
}

pub fn camera_description() -> CameraDescription {
    CameraDescription {
        make: "OnePlus".to_owned(),
        model: "CPH2447".to_owned(),
        unique_camera_model: "CPH2447-OnePlus-OnePlus camera 2".to_owned(),
        software: "Fasticamera 0.1".to_owned(),
        lens: Some(LensDescription {
            focal_length_mm: 5.59,
            f_number: 1.8,
        }),
    }
}

/// 2026-10-25 20:48:05.250 по UTC+03:00 (эталонный кадр снят в этот момент).
pub fn capture_moment() -> CaptureMoment {
    CaptureMoment {
        utc_millis: 1_792_950_485_250,
        utc_offset_minutes: 180,
    }
}
