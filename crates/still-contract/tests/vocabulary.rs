//! Поведение словаря снимка: проверки размеров буферов, повороты, итоговость событий.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "тесты: падение и есть провал")]

use std::sync::Arc;

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::geometry::PixelSize;
use shell_contract::clock::MonotonicNanos;
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::frame_choice::{FrameChoice, NoFrameReason, SubstitutionReason};
use still_contract::phase::StillFailure;
use still_contract::plan::{PrimaryFormat, RawOutput, StillId, StillOutputs};
use still_contract::shot::{OwnedPixels, RawSamples16, Rgba8Image, StillRotation};

#[test]
fn raw_samples_read_little_endian_and_reject_wrong_storage_size() {
    // 2×2 кадр: отсчёты 1, 2, 1023, 64 — младший байт первым (TIFF `II`).
    let bytes = vec![1, 0, 2, 0, 0xFF, 0x03, 64, 0];
    let raw = RawSamples16::new(PixelSize::new(2, 2), Arc::new(OwnedPixels(bytes))).unwrap();
    assert_eq!(raw.sample(0, 0), Some(1));
    assert_eq!(raw.sample(1, 0), Some(2));
    assert_eq!(raw.sample(0, 1), Some(1023));
    assert_eq!(raw.sample(1, 1), Some(64));
    assert_eq!(raw.sample(2, 0), None, "вне кадра — нет отсчёта, а не чужие байты");

    let short = RawSamples16::new(PixelSize::new(2, 2), Arc::new(OwnedPixels(vec![0; 7])));
    let error = short.unwrap_err();
    assert_eq!((error.expected, error.actual), (8, 7));
}

#[test]
fn rgba_image_requires_four_bytes_per_pixel() {
    assert!(Rgba8Image::new(PixelSize::new(3, 1), Arc::new(OwnedPixels(vec![0; 12]))).is_ok());
    let error = Rgba8Image::new(PixelSize::new(3, 1), Arc::new(OwnedPixels(vec![0; 9]))).unwrap_err();
    assert_eq!(error.expected, 12);
}

#[test]
fn rotation_accepts_only_multiples_of_ninety_and_wraps() {
    assert_eq!(StillRotation::from_degrees(0), Some(StillRotation::None));
    assert_eq!(StillRotation::from_degrees(450), Some(StillRotation::Clockwise90));
    assert_eq!(StillRotation::from_degrees(-90), Some(StillRotation::Clockwise270));
    assert_eq!(StillRotation::from_degrees(180).map(StillRotation::degrees), Some(180));
    assert_eq!(StillRotation::from_degrees(45), None);
}

#[test]
fn only_secured_is_not_final() {
    let id = StillId(7);
    let finals = [
        DevelopEvent::NotSecured {
            id,
            reason: NoFrameReason::RingEmpty,
        },
        DevelopEvent::PrimaryRendered { id },
        DevelopEvent::Failed {
            id,
            failure: DevelopFailure::Abandoned,
        },
    ];
    for event in finals {
        assert!(event.is_final(), "{event:?}");
        assert_eq!(event.id(), id);
    }
}

#[test]
fn frame_choice_reports_chosen_timestamp_and_exactness() {
    let exact = FrameChoice::ShownAtPress {
        sensor_timestamp: SensorTimestamp::from_nanos(10),
        shown_at: MonotonicNanos::from_nanos(99),
    };
    let substituted = FrameChoice::Substituted {
        sensor_timestamp: SensorTimestamp::from_nanos(20),
        wanted: Some(SensorTimestamp::from_nanos(10)),
        reason: SubstitutionReason::ShownFrameEvicted,
    };
    assert!(exact.is_exact());
    assert!(!substituted.is_exact());
    assert_eq!(substituted.sensor_timestamp(), SensorTimestamp::from_nanos(20));
}

#[test]
fn outputs_need_raw_samples_only_with_dng() {
    let heif_only = StillOutputs {
        primary: PrimaryFormat::Heif,
        raw: RawOutput::Skip,
    };
    let with_dng = StillOutputs {
        raw: RawOutput::Dng,
        ..heif_only
    };
    assert!(!heif_only.needs_raw_samples());
    assert!(with_dng.needs_raw_samples());
    assert_eq!(PrimaryFormat::Heif.mime_type(), "image/heic");
    assert_eq!(PrimaryFormat::Jpeg.file_extension(), "JPG");
}

#[test]
fn develop_failure_becomes_still_failure_with_text() {
    let failure: StillFailure = DevelopFailure::Gpu("device lost".to_owned()).into();
    assert_eq!(
        failure,
        StillFailure::Develop("ошибка GPU при проявлении: device lost".to_owned())
    );
}
