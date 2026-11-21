//! Условие «экран показал живую картинку активной камеры»: каждый случай даёт другой ответ при поломке условия.

use camera_contract::fixtures::camera_id;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::viewfinder_crop::{CropAspect, CropCenter, ViewfinderCrop, ZoomFactor};
use camera_contract::zoom::{FramedCrop, PreviewMirror, ZoomFit, ZoomRatio};
use render_loop::viewfinder::{BlackKind, FrameCropReport, TransitionKind};
use shell_contract::clock::MonotonicNanos;
use ui_draw::frame_source::BackdropContinuity;

use super::*;

/// Кадр с заданной меткой и (необязательно) камерой кадрирования.
fn frame(label: FrameLabel, camera: Option<&str>) -> FrameRecord {
    FrameRecord {
        present_time: MonotonicNanos::from_nanos(1_000_000),
        label,
        backdrop: BackdropContinuity::Live,
        crop: camera.map(|name| FrameCropReport {
            camera: camera_id(name),
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
        }),
        held_magnification: 1.0,
    }
}

#[test]
fn a_live_frame_of_the_active_camera_means_the_picture_is_on_screen() {
    let frames = [frame(FrameLabel::Live, Some("0"))];

    assert!(screen_shows_live_picture_of(&frames, &camera_id("0")));
}

#[test]
fn a_live_frame_among_older_non_live_frames_still_counts() {
    let frames = [
        frame(FrameLabel::Black(BlackKind::PictureExpected), None),
        frame(FrameLabel::Live, Some("0")),
    ];

    assert!(screen_shows_live_picture_of(&frames, &camera_id("0")));
}

#[test]
fn a_live_frame_of_another_camera_is_not_the_active_picture() {
    let frames = [frame(FrameLabel::Live, Some("2"))];

    assert!(!screen_shows_live_picture_of(&frames, &camera_id("0")));
}

#[test]
fn held_black_and_recomposed_frames_are_not_a_live_picture() {
    let frames = [
        frame(FrameLabel::Transition(TransitionKind::HeldLastFrame), Some("0")),
        frame(FrameLabel::Black(BlackKind::PictureExpected), Some("0")),
        frame(FrameLabel::Black(BlackKind::PictureUnavailable), None),
        frame(FrameLabel::Live, None),
    ];

    assert!(!screen_shows_live_picture_of(&frames, &camera_id("0")));
}

#[test]
fn no_frames_at_all_is_not_a_live_picture() {
    assert!(!screen_shows_live_picture_of(&[], &camera_id("0")));
}
