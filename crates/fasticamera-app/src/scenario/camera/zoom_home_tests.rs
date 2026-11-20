//! Приведение к дому и суд над чужими событиями: чистые правила на подставных срезах.

use std::sync::Arc;

use camera_core::lens_status::LensSwitchFailure;

use super::*;
use crate::camera_screen_effects::RecentHapticTimes;
use crate::camera_test_support::oneplus_zoom_catalog;
use crate::scenario::camera::zoom_views::ActiveLensView;

fn engine(role: LensRole, facing: LensFacing) -> EngineView {
    EngineView {
        streaming: true,
        active: Some(ActiveLensView {
            role,
            camera: camera_contract::fixtures::camera_id("2"),
            facing,
        }),
        catalog: Arc::clone(&oneplus_zoom_catalog()),
        measured_fps: Some(60.0),
    }
}

fn screen(ratio_milli: u32, facing: LensFacing, wheel_open: bool, gliding: bool) -> ScreenView {
    ScreenView {
        zoom: ZoomView {
            ratio_milli,
            facing,
            wheel_open,
            gliding,
        },
        haptics: RecentHapticTimes::default(),
        lens_published: 0,
    }
}

fn names(steps: &[ZoomStep]) -> Vec<&str> {
    steps.iter().map(|step| step.name.as_str()).collect()
}

#[test]
fn a_screen_already_at_back_one_x_needs_no_steps() {
    let steps = steps_to_home(
        &engine(LensRole::Wide, LensFacing::Back),
        &screen(1_000, LensFacing::Back, false, false),
        "p",
    );
    assert!(steps.is_empty(), "{steps:?}");
    // В допуске 2 % — тоже дом.
    let near = steps_to_home(
        &engine(LensRole::Wide, LensFacing::Back),
        &screen(1_015, LensFacing::Back, false, false),
        "p",
    );
    assert!(near.is_empty());
}

#[test]
fn the_front_camera_is_flipped_back_and_then_one_x_is_tapped() {
    let steps = steps_to_home(
        &engine(LensRole::Front, LensFacing::Front),
        &screen(1_000, LensFacing::Front, false, false),
        "p",
    );
    assert_eq!(names(&steps), vec!["p_flip_to_back", "p_tap_1"]);
    assert_eq!(steps[0].action, ZoomAction::Flip);
    assert_eq!(steps[1].action, ZoomAction::TapStop { ratio: 1.0 });
}

#[test]
fn every_other_deviation_from_home_is_fixed_by_one_tap_on_one_x() {
    for (label, engine, screen) in [
        (
            "ширик",
            engine(LensRole::UltraWide, LensFacing::Back),
            screen(590, LensFacing::Back, false, false),
        ),
        (
            "зум 5×",
            engine(LensRole::Wide, LensFacing::Back),
            screen(5_000, LensFacing::Back, false, false),
        ),
        (
            "колесо открыто",
            engine(LensRole::Wide, LensFacing::Back),
            screen(1_000, LensFacing::Back, true, false),
        ),
        (
            "идёт переход",
            engine(LensRole::Wide, LensFacing::Back),
            screen(1_000, LensFacing::Back, false, true),
        ),
    ] {
        let steps = steps_to_home(&engine, &screen, "p");
        assert_eq!(names(&steps), vec!["p_tap_1"], "{label}");
    }
}

#[test]
fn only_failed_and_unrequested_lens_events_are_stray() {
    let failed = CameraEvent::LensSwitchFailed {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
        reason: LensSwitchFailure::Unavailable {
            reason: camera_contract::availability::CameraUnavailableReason::DeviceFailure,
        },
    };
    let text = stray_event_failure(&[failed], "после шага 03").expect("провал");
    assert!(
        text.starts_with("после шага 03:") && text.contains("wide→ultra_wide") && text.contains("не удалась"),
        "{text}"
    );

    let switched = CameraEvent::LensSwitched {
        from: LensRole::Wide,
        to: LensRole::Tele,
        elapsed: std::time::Duration::from_millis(300),
    };
    let text = stray_event_failure(&[switched], "после шага 03").expect("провал");
    assert!(text.contains("wide→tele") && text.contains("ни один шаг"), "{text}");

    assert_eq!(stray_event_failure(&[], "x"), None);
    let retarget = CameraEvent::LensSwitchRetargeted {
        from: LensRole::Wide,
        abandoned: LensRole::UltraWide,
        to: LensRole::Tele,
    };
    assert_eq!(
        stray_event_failure(&[retarget], "x"),
        None,
        "повторная цель — не провал"
    );
}
