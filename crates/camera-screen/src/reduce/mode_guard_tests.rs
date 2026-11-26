//! Тесты охраны по режиму: действия управления камерой в CALIBRATE отвергаются самим редьюсером (а не только тем,
//! что сцена не рисует для них виджетов), а в PHOTO работают как раньше.

use camera_contract::control::ExposureBias;

use crate::action::ScreenAction;
use crate::look_test_support::look_facts;
use crate::mode_behaviour::ModeBehaviour;
use crate::outcome::{ActionOutcome, Rejection};
use crate::test_support::{Screen, at_ms, ratio};
use crate::tools::ToolId;
use ui_core::geometry::Point;

/// По одному действию из каждого семейства управления камерой: фокус, LOCK, экспозиция, значки, зум.
fn camera_control_actions() -> Vec<(&'static str, ScreenAction)> {
    let at = at_ms(10);
    vec![
        (
            "тап по кадру",
            ScreenAction::ViewfinderTapped {
                point: Point::new(200.0, 400.0),
                at,
            },
        ),
        (
            "долгий тап (LOCK)",
            ScreenAction::ViewfinderLongPressed {
                point: Point::new(200.0, 400.0),
                at,
            },
        ),
        (
            "перетаскивание экспозиции",
            ScreenAction::ExposureDragged {
                requested: ExposureBias::from_ev(1.0).expect("EV"),
                at,
            },
        ),
        ("палец на «солнце»", ScreenAction::FocusSunHeld { at }),
        ("сброс экспозиции", ScreenAction::ExposureResetRequested),
        ("значок над кадром", ScreenAction::StripToolTapped(ToolId::Timer)),
        ("упор зума", ScreenAction::ZoomStopTapped { stop: 0, at }),
        ("нажатие на ленту зума", ScreenAction::ZoomStripPressBegan { at }),
        ("касание колеса зума", ScreenAction::ZoomWheelTouched { at }),
        (
            "перетаскивание колеса зума",
            ScreenAction::ZoomWheelDragged {
                requested: ratio(2.0),
                at,
            },
        ),
        ("начало щипка", ScreenAction::ZoomPinchBegan { at }),
        ("изменение щипка", ScreenAction::ZoomPinchChanged { scale: 1.5, at }),
    ]
}

#[test]
fn camera_control_actions_are_rejected_in_calibrate_without_effects_or_state_changes() {
    for (name, action) in camera_control_actions() {
        let mut screen = Screen::calibrating(look_facts());
        let before = screen.state.clone();

        let transition = screen.step(action);

        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{name}"
        );
        assert!(transition.effects.is_empty(), "{name}: {:?}", transition.effects);
        assert_eq!(
            screen.state.rejections().get(Rejection::NotAvailableInMode),
            1,
            "{name}: отказ посчитан"
        );
        // Кроме счётчика отказов и метки времени, состояние то же.
        let mut after = screen.state.clone();
        after.rejections = before.rejections;
        after.last_time = before.last_time;
        assert_eq!(after, before, "{name}: состояние не тронуто");
    }
}

/// Белое, которого у раздела калибровки нет (решение 294): «Auto» и долгое нажатие на кнопку у затвора.
fn full_white_actions() -> Vec<(&'static str, ScreenAction)> {
    vec![
        ("долгое нажатие на кнопку белого", ScreenAction::WhiteButtonLongPressed),
        ("тап «Auto»", ScreenAction::WhiteAutoTapped),
    ]
}

#[test]
fn auto_and_the_shutter_long_press_are_rejected_in_calibrate_without_effects_or_state_changes() {
    for (name, action) in full_white_actions() {
        let mut screen = Screen::calibrating(look_facts());
        let before = screen.state.clone();

        let transition = screen.step(action);

        assert_eq!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{name}"
        );
        assert!(transition.effects.is_empty(), "{name}: {:?}", transition.effects);
        let mut after = screen.state.clone();
        after.rejections = before.rejections;
        after.last_time = before.last_time;
        assert_eq!(after, before, "{name}: состояние не тронуто");
    }
}

#[test]
fn auto_and_the_shutter_long_press_still_work_in_photo() {
    for (name, action) in full_white_actions() {
        let mut screen = Screen::ready();
        let transition = screen.step(action);
        assert_ne!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{name}"
        );
    }
}

#[test]
fn camera_control_actions_are_not_blocked_by_the_mode_guard_in_photo() {
    for (name, action) in camera_control_actions() {
        let mut screen = Screen::ready();

        let transition = screen.step(action);

        assert_ne!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{name}"
        );
        assert_eq!(
            screen.state.rejections().get(Rejection::NotAvailableInMode),
            0,
            "{name}"
        );
    }
}

#[test]
fn a_tap_in_photo_still_aims_the_focus() {
    let mut screen = Screen::ready();

    let tap = screen.tap_center(at_ms(10));

    assert_eq!(tap.outcome, ActionOutcome::Applied);
    assert!(!tap.effects.is_empty(), "фокус в PHOTO наводится");
}

#[test]
fn gesture_releases_still_pass_in_calibrate_so_a_gesture_begun_in_photo_can_finish() {
    let mut screen = Screen::calibrating(look_facts());
    for action in [
        ScreenAction::ZoomStripPressEnded { at: at_ms(10) },
        ScreenAction::ZoomWheelReleased { at: at_ms(10) },
        ScreenAction::ZoomPinchEnded { at: at_ms(10) },
    ] {
        let transition = screen.step(action.clone());
        assert_ne!(
            transition.outcome,
            ActionOutcome::Rejected(Rejection::NotAvailableInMode),
            "{action:?}"
        );
    }
}

#[test]
fn camera_controls_are_data_of_the_mode_not_a_mode_check() {
    assert!(ModeBehaviour::PHOTO.has_camera_controls());
    assert!(!ModeBehaviour::CALIBRATE.has_camera_controls());
}

#[test]
fn white_controls_and_tracking_hold_are_data_of_the_mode() {
    use crate::mode_behaviour::{WhiteControls, WhiteTrackingHold};
    assert_eq!(ModeBehaviour::PHOTO.white_controls, WhiteControls::Full);
    assert_eq!(ModeBehaviour::PHOTO.white_tracking, WhiteTrackingHold::Running);
    assert_eq!(ModeBehaviour::CALIBRATE.white_controls, WhiteControls::ManualOnly);
    assert_eq!(
        ModeBehaviour::CALIBRATE.white_tracking,
        WhiteTrackingHold::PausedForCalibration
    );
}
