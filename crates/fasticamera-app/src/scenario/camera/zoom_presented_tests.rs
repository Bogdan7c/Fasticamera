//! Гистограмма показанных кадров и приращения счётчиков: каждое значение изменилось бы при поломке разбора.

use camera_contract::fixtures::camera_id;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::viewfinder_crop::{CropAspect, CropCenter, ViewfinderCrop, ZoomFactor};
use camera_contract::zoom::{FramedCrop, PreviewMirror, ZoomFit, ZoomRatio};
use render_loop::viewfinder::FrameCropReport;
use ui_draw::frame_source::BackdropContinuity;

use super::*;

fn at_ms(ms: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(ms * 1_000_000)
}

fn crop_of(camera: &str) -> FrameCropReport {
    FrameCropReport {
        camera: camera_id(camera),
        resolved: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(4096, 3072),
        },
        framed: FramedCrop {
            crop: ViewfinderCrop {
                aspect: CropAspect::SensorNative,
                zoom: ZoomFactor::NONE,
                center: CropCenter::ValidAreaCenter,
            },
            mirror: PreviewMirror::None,
            shown_ratio: ZoomRatio::ONE,
            fit: ZoomFit::Exact,
        },
    }
}

fn frame(ms: i64, label: FrameLabel, camera: Option<&str>) -> FrameRecord {
    FrameRecord {
        present_time: at_ms(ms),
        label,
        backdrop: BackdropContinuity::Live,
        crop: camera.map(crop_of),
        held_magnification: 1.0,
    }
}

#[test]
fn histogram_splits_frames_by_label_and_camera_and_reports_the_first_frame_offset() {
    let frames = vec![
        frame(50, FrameLabel::Live, Some("2")), // до действия: не считается
        frame(110, FrameLabel::Black(BlackKind::NoPicture), None),
        frame(126, FrameLabel::Black(BlackKind::NoPicture), None),
        frame(142, FrameLabel::Live, Some("2")), // старая камера
        frame(400, FrameLabel::Live, Some("3")),
        frame(416, FrameLabel::Live, Some("3")),
        frame(432, FrameLabel::Live, Some("3")),
    ];
    let histogram = presented_histogram(&frames, at_ms(100));
    assert_eq!(
        histogram,
        vec![
            PresentedBucket {
                label: "black_no_picture",
                camera: None,
                frames: 2,
                first_at_ms: 10.0
            },
            PresentedBucket {
                label: "live",
                camera: Some("2".to_owned()),
                frames: 1,
                first_at_ms: 42.0
            },
            PresentedBucket {
                label: "live",
                camera: Some("3".to_owned()),
                frames: 3,
                first_at_ms: 300.0
            },
        ]
    );
}

#[test]
fn histogram_of_an_empty_window_is_empty() {
    assert!(presented_histogram(&[frame(50, FrameLabel::Live, Some("2"))], at_ms(100)).is_empty());
}

#[test]
fn label_names_are_distinct_for_every_label() {
    let labels = [
        FrameLabel::Live,
        FrameLabel::Transition(TransitionKind::HeldLastFrame),
        FrameLabel::Black(BlackKind::NoPicture),
        FrameLabel::Black(BlackKind::PictureExpected),
        FrameLabel::Black(BlackKind::CameraUnavailable),
        FrameLabel::Black(BlackKind::PictureUnavailable),
    ];
    let names: std::collections::BTreeSet<&str> = labels.into_iter().map(label_name).collect();
    assert_eq!(names.len(), labels.len());
}

#[test]
fn delta_counts_growth_of_continuity_and_isp_counters_only() {
    let start = ContinuityCounters {
        black_frames_before_first_picture: 4,
        live_frames_presented: 100,
        holds_started: 2,
        ..ContinuityCounters::default()
    };
    let end = ContinuityCounters {
        black_frames_before_first_picture: 7,
        live_frames_presented: 130,
        holds_started: 4,
        holds_started_exposure_without_target: 1,
        holds_started_exposure_level_off: 1,
        frames_hidden_by_continuity: 5,
        ..ContinuityCounters::default()
    };
    let isp_start = ViewfinderCounters {
        frames_hidden_cold_start: 1,
        recomposed: 10,
        ..ViewfinderCounters::default()
    };
    let isp_end = ViewfinderCounters {
        frames_hidden_cold_start: 4,
        recomposed: 10,
        ..ViewfinderCounters::default()
    };
    let delta = ContinuityDelta::between(&start, &end, &isp_start, &isp_end);
    assert_eq!(
        delta,
        ContinuityDelta {
            black_frames_before_first_picture: 3,
            live_frames_presented: 30,
            frames_hidden_by_continuity: 5,
            frames_hidden_cold_start: 3,
            holds_started: 2,
            hold_causes: HoldCauses {
                exposure_without_target: 1,
                exposure_level_off: 1,
                ..HoldCauses::default()
            },
            ..ContinuityDelta::default()
        }
    );
}
