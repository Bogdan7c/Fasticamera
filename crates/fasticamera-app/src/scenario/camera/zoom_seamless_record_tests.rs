//! Разрыв показа при смене камеры: значения считаются по кадрам обеих камер на одной шкале.

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

fn live(ms: i64, camera: &str) -> FrameRecord {
    FrameRecord {
        present_time: at_ms(ms),
        label: FrameLabel::Live,
        backdrop: BackdropContinuity::Live,
        crop: Some(FrameCropReport {
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
        }),
        held_magnification: 1.0,
    }
}

/// Старая камера «2» идёт каждые 17 мс до 170, новая «3» начинается в `first_new_ms`.
fn timeline(first_new_ms: i64) -> Vec<FrameRecord> {
    let mut frames: Vec<FrameRecord> = (0..=10).map(|index| live(index * 17, "2")).collect();
    frames.extend((0..4).map(|index| live(first_new_ms + index * 17, "3")));
    frames
}

#[test]
fn a_gap_of_one_period_means_the_new_camera_took_the_next_slot() {
    let gap = switch_gap(&timeline(187), &camera_id("2"), &camera_id("3"), at_ms(0)).expect("разрыв измерен");
    assert!((gap.period_ms - 17.0).abs() < 1e-9, "{gap:?}");
    assert!((gap.gap_ms - 17.0).abs() < 1e-9, "{gap:?}");
    assert!((gap.periods() - 1.0).abs() < 1e-9);
}

#[test]
fn a_stall_shows_up_as_many_periods() {
    let gap = switch_gap(&timeline(170 + 85), &camera_id("2"), &camera_id("3"), at_ms(0)).expect("разрыв измерен");
    assert!((gap.periods() - 5.0).abs() < 1e-9, "{gap:?}");
}

#[test]
fn new_camera_frames_before_the_window_do_not_count_as_the_switch() {
    let mut frames = vec![live(-400, "3")];
    frames.extend(timeline(187));
    let since = at_ms(100);
    let gap = switch_gap(&frames, &camera_id("2"), &camera_id("3"), since).expect("разрыв измерен");
    assert!((gap.gap_ms - 17.0).abs() < 1e-9, "{gap:?}");
}

#[test]
fn without_enough_old_frames_or_without_the_new_camera_nothing_is_measured() {
    let frames = vec![live(0, "2"), live(17, "2"), live(60, "3")];
    assert_eq!(switch_gap(&frames, &camera_id("2"), &camera_id("3"), at_ms(0)), None);
    assert_eq!(
        switch_gap(&timeline(187)[..11], &camera_id("2"), &camera_id("3"), at_ms(0)),
        None,
        "кадров новой камеры нет"
    );
}

#[test]
fn a_recomposed_frame_between_the_cameras_is_not_a_stall() {
    let mut frames = timeline(170 + 85);
    let mut recomposed = live(187, "2");
    recomposed.crop = None;
    frames.push(recomposed);
    frames.sort_by_key(|frame| frame.present_time);
    let gap = switch_gap(&frames, &camera_id("2"), &camera_id("3"), at_ms(0)).expect("разрыв измерен");
    assert!(
        (gap.gap_ms - 68.0).abs() < 1e-9,
        "до пересобранного кадра, а не до последнего кадра старой камеры: {gap:?}"
    );
}

#[test]
fn the_fps_pair_reports_the_slower_camera() {
    let pair = FpsPair {
        primary: 59.9,
        standby: 54.2,
    };
    assert_eq!(pair.minimum(), 54.2);
}
