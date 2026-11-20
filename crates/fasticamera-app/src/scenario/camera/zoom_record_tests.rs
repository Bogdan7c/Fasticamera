//! Разборы наблюдений сценария `zoom` на подставных кадрах и счётчиках: каждое правило проверяется значением,
//! которое изменилось бы при поломке.

use camera_contract::fixtures::camera_id;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::viewfinder_crop::{CropAspect, CropCenter, ViewfinderCrop, ZoomFactor};
use camera_contract::zoom::{FramedCrop, ZoomFit, ZoomRatio};
use render_loop::viewfinder::{BlackKind, FrameCropReport, TransitionKind};
use ui_draw::frame_source::BackdropContinuity;

use super::*;

fn at_ms(ms: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(ms * 1_000_000)
}

fn crop_report(camera: &str, mirror: PreviewMirror) -> FrameCropReport {
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
            mirror,
            shown_ratio: ZoomRatio::ONE,
            fit: ZoomFit::Exact,
        },
    }
}

fn live(ms: i64, camera: &str, mirror: PreviewMirror) -> FrameRecord {
    FrameRecord {
        present_time: at_ms(ms),
        label: FrameLabel::Live,
        backdrop: BackdropContinuity::Live,
        crop: Some(crop_report(camera, mirror)),
        held_magnification: 1.0,
    }
}

fn held(ms: i64) -> FrameRecord {
    FrameRecord {
        present_time: at_ms(ms),
        label: FrameLabel::Transition(TransitionKind::HeldLastFrame),
        backdrop: BackdropContinuity::HeldLastFrame { since: at_ms(ms) },
        crop: None,
        held_magnification: 1.0,
    }
}

#[test]
fn first_live_frame_is_the_earliest_live_frame_of_the_new_camera_after_the_action() {
    let frames = vec![
        live(90, "2", PreviewMirror::None),  // до действия, старая камера
        held(110),                           // удержанный кадр — не живой
        live(130, "2", PreviewMirror::None), // живой, но старая камера
        live(250, "3", PreviewMirror::None), // первый живой новой камеры
        live(266, "3", PreviewMirror::None),
    ];
    assert_eq!(first_live_frame_ms(&frames, &camera_id("3"), at_ms(100)), Some(150.0));
}

#[test]
fn a_live_frame_of_the_new_camera_before_the_action_does_not_count() {
    let frames = vec![live(50, "3", PreviewMirror::None)];
    assert_eq!(first_live_frame_ms(&frames, &camera_id("3"), at_ms(100)), None);
}

#[test]
fn black_and_held_frames_never_count_as_the_first_live_frame() {
    let black = FrameRecord {
        label: FrameLabel::Black(BlackKind::PictureExpected),
        crop: Some(crop_report("3", PreviewMirror::None)),
        ..held(200)
    };
    assert_eq!(
        first_live_frame_ms(&[held(150), black], &camera_id("3"), at_ms(100)),
        None
    );
}

#[test]
fn mirror_is_correct_for_a_mirrored_front_and_a_plain_back_and_wrong_when_swapped() {
    let front = vec![
        live(200, "1", PreviewMirror::Horizontal),
        live(216, "1", PreviewMirror::Horizontal),
    ];
    assert_eq!(
        mirror_check(&front, &camera_id("1"), at_ms(100), LensFacing::Front),
        MirrorCheck::Correct
    );
    let back = vec![live(200, "2", PreviewMirror::None)];
    assert_eq!(
        mirror_check(&back, &camera_id("2"), at_ms(100), LensFacing::Back),
        MirrorCheck::Correct
    );
    let unmirrored_front = vec![
        live(200, "1", PreviewMirror::None),
        live(216, "1", PreviewMirror::Horizontal),
    ];
    assert_eq!(
        mirror_check(&unmirrored_front, &camera_id("1"), at_ms(100), LensFacing::Front),
        MirrorCheck::Wrong { frames: 1 },
        "один кадр фронталки без зеркала — ошибка, и она считается"
    );
    let mirrored_back = vec![live(200, "2", PreviewMirror::Horizontal)];
    assert_eq!(
        mirror_check(&mirrored_back, &camera_id("2"), at_ms(100), LensFacing::Back),
        MirrorCheck::Wrong { frames: 1 }
    );
}

#[test]
fn mirror_without_frames_of_the_camera_is_not_measured_rather_than_correct() {
    let frames = vec![live(200, "2", PreviewMirror::None)];
    assert_eq!(
        mirror_check(&frames, &camera_id("1"), at_ms(100), LensFacing::Front),
        MirrorCheck::NoFrameObserved
    );
}

#[test]
fn ring_watch_sees_a_hole_only_when_the_next_snapshot_starts_after_the_last_seen_frame() {
    let mut watch = RingWatch::default();
    watch.absorb(&[live(100, "2", PreviewMirror::None), live(116, "2", PreviewMirror::None)]);
    assert!(!watch.overran(), "первый снимок дыры не создаёт");
    // Перекрытие со старым снимком: ничего не потеряно.
    watch.absorb(&[live(116, "2", PreviewMirror::None), live(132, "2", PreviewMirror::None)]);
    assert!(!watch.overran());
    // Новый снимок начинается позже последнего виденного: между ними кадры, которых мы не видели.
    watch.absorb(&[live(400, "2", PreviewMirror::None)]);
    assert!(watch.overran());
    watch.reset_overrun();
    assert!(!watch.overran());
}

#[test]
fn an_empty_ring_changes_nothing() {
    let mut watch = RingWatch::default();
    watch.absorb(&[]);
    watch.absorb(&[live(100, "2", PreviewMirror::None)]);
    assert!(!watch.overran());
}

#[test]
fn haptic_window_counts_only_the_window_and_measures_its_length() {
    // К началу окна принято 2 вибрации, к концу — 6; окно 300..1500 мс.
    let window = haptic_window(6, 2, at_ms(300), at_ms(1_500));
    assert_eq!(window.count, 4);
    assert_eq!(window.window_ms, 1_200.0);
}

#[test]
fn a_counter_that_went_backwards_gives_zero_haptics_not_a_wrapped_number() {
    let window = haptic_window(1, 3, at_ms(0), at_ms(10));
    assert_eq!(window.count, 0);
}
