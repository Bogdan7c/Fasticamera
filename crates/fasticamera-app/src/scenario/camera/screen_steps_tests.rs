//! План шагов `screen`: порядок, уникальные имена, жесты из плана касаний, нейтральный конец.

use super::*;
use crate::camera_screen_source::{ModeKind, PanelKind, ShutterPlacement};
use crate::screen_scenario_config::ScreenTouchPlan;
use camera_screen::WhiteScreenConfig;

#[test]
fn step_names_are_unique_and_the_order_builds_state_before_it_is_probed() {
    let steps = planned_steps(&ScreenTouchPlan::default(), &WhiteScreenConfig::default());
    let names: Vec<&str> = steps.iter().map(|step| step.name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "имена шагов повторяются: {names:?}");
    let position = |name: &str| names.iter().position(|candidate| *candidate == name).expect(name);
    assert!(position("lock_long_press") < position("unlock_tap"));
    assert!(position("menu_open_swipe") < position("menu_probe_tap_opened"));
    assert!(position("menu_close_swipe") < position("menu_probe_tap_closed"));
    assert!(position("ev_icon_tap") < position("ev_slider_swipe"));
    assert!(position("ev_slider_swipe") < position("ev_reset_tap"));
    assert!(position("ev_close_tap") < position("ev_probe_tap_closed"));
    assert_eq!(names.last(), Some(&"volume_up_shutter"));
}

#[test]
fn gestures_come_from_the_touch_plan_not_from_literals() {
    let plan = ScreenTouchPlan {
        focus_center: crate::screen_scenario_config::ScreenPoint::new(0.11, 0.22),
        ..ScreenTouchPlan::default()
    };
    let steps = planned_steps(&plan, &WhiteScreenConfig::default());
    assert_eq!(steps[0].gesture, Gesture::Tap { x: 0.11, y: 0.22 });
}

#[test]
fn the_plan_ends_neutral_the_screen_is_unlocked_and_the_exposure_reset() {
    use super::super::screen_expect::StateCheck;
    use crate::camera_screen_source::Presence;
    use camera_contract::three_a::AeAfLock;
    let steps = planned_steps(&ScreenTouchPlan::default(), &WhiteScreenConfig::default());
    let find = |name: &str| steps.iter().find(|step| step.name == name).expect(name);
    assert_eq!(
        find("lock_long_press").expect.states(),
        [
            StateCheck::Lock(AeAfLock::Locked),
            StateCheck::LockPlaque(Presence::Shown)
        ]
    );
    assert_eq!(
        find("unlock_tap").expect.states(),
        [
            StateCheck::Lock(AeAfLock::Unlocked),
            StateCheck::LockPlaque(Presence::Hidden)
        ]
    );
    assert_eq!(
        find("ev_slider_swipe").expect.states(),
        [StateCheck::ExposureBiasIsSet, StateCheck::ScreenExposureStepsAreSet]
    );
    assert_eq!(
        find("ev_reset_tap").expect.states(),
        [StateCheck::ExposureBiasIsZero, StateCheck::ScreenExposureStepsAreZero]
    );
}

#[test]
fn every_inferred_step_says_how_it_is_inferred() {
    for step in planned_steps(&ScreenTouchPlan::default(), &WhiteScreenConfig::default()) {
        if let Some(reason) = step.inferred {
            assert!(!reason.is_empty(), "{}", step.name);
        }
        assert!(!step.purpose.is_empty(), "{}", step.name);
    }
}

#[test]
fn no_step_is_left_inferred_and_the_state_steps_check_the_screen_directly() {
    let steps = planned_steps(&ScreenTouchPlan::default(), &WhiteScreenConfig::default());
    assert!(steps.iter().all(|step| step.inferred.is_none()));
    let find = |name: &str| steps.iter().find(|step| step.name == name).expect(name);
    let panel_of = |name: &str| {
        find(name).expect.states().iter().find_map(|check| match check {
            StateCheck::Panel(panel) => Some(*panel),
            _ => None,
        })
    };
    assert_eq!(panel_of("menu_open_swipe"), Some(PanelKind::ControlsMenu));
    assert_eq!(panel_of("menu_close_swipe"), Some(PanelKind::Closed));
    assert_eq!(panel_of("ev_icon_tap"), Some(PanelKind::ExposurePanel));
    assert_eq!(panel_of("ev_close_tap"), Some(PanelKind::Closed));
    let shutter_of = |name: &str| {
        find(name).expect.states().iter().find_map(|check| match check {
            StateCheck::Shutter(placement) => Some(*placement),
            _ => None,
        })
    };
    assert_eq!(shutter_of("ev_icon_tap"), Some(ShutterPlacement::RaisedAboveCard));
    assert_eq!(shutter_of("ev_close_tap"), Some(ShutterPlacement::Visible));
    assert_eq!(shutter_of("menu_open_swipe"), Some(ShutterPlacement::HiddenUnderDrawer));
    assert!(
        find("modes_swipe_left")
            .expect
            .states()
            .contains(&StateCheck::Mode(ModeKind::Calibrate))
    );
    assert!(
        find("modes_swipe_right")
            .expect
            .states()
            .contains(&StateCheck::Mode(ModeKind::Photo))
    );
}

#[test]
fn the_white_steps_open_the_card_set_a_light_close_it_and_long_press_in_that_order() {
    let steps = planned_steps(&ScreenTouchPlan::default(), &WhiteScreenConfig::default());
    let names: Vec<&str> = steps.iter().map(|step| step.name).collect();
    let position = |name: &str| names.iter().position(|candidate| *candidate == name).expect(name);
    assert!(position("white_button_tap") < position("white_light_tap"));
    assert!(position("white_light_tap") < position("white_card_close_tap"));
    assert!(position("white_card_close_tap") < position("white_long_press"));
    // Экран возвращён в исходное состояние до шагов EV: им нужна закрытая панель.
    assert!(position("white_long_press") < position("ev_icon_tap"));
}

#[test]
fn the_light_step_expects_the_numbers_of_the_configured_button_not_a_copied_literal() {
    use super::super::screen_white_check::WhiteCheck;
    let mut white = WhiteScreenConfig::default();
    white.lights[0].kelvin = 4_321.0;
    white.lights[0].duv = 0.0123;
    let steps = planned_steps(&ScreenTouchPlan::default(), &white);
    let light = steps.iter().find(|step| step.name == "white_light_tap").expect("шаг");
    assert!(
        light
            .expect
            .states()
            .contains(&StateCheck::White(WhiteCheck::SetByHand {
                kelvin: 4_321.0,
                duv: 0.0123,
            }))
    );
}

#[test]
fn the_white_button_and_the_ev_icon_are_distinct_taps_and_the_long_press_uses_the_white_button() {
    let plan = ScreenTouchPlan::default();
    let steps = planned_steps(&plan, &WhiteScreenConfig::default());
    let gesture_of = |name: &str| steps.iter().find(|step| step.name == name).expect(name).gesture;
    assert_eq!(gesture_of("white_button_tap"), Gesture::tap(plan.white_button));
    assert_eq!(gesture_of("ev_icon_tap"), Gesture::tap(plan.ev_icon));
    assert_ne!(plan.white_button, plan.ev_icon);
    assert_eq!(gesture_of("white_long_press"), Gesture::long_press(plan.white_hold));
    assert_eq!(
        plan.white_hold.at, plan.white_button,
        "долгое нажатие идёт по кнопке белого"
    );
}
