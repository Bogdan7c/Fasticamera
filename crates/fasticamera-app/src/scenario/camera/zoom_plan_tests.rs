//! План `zoom`: порядок шагов, ожидания смены объектива, поиск кнопки ленты.

use camera_contract::zoom::ZoomStopKind;

use super::*;
use crate::camera_test_support::oneplus_zoom_catalog;

fn back_group() -> LensGroup {
    oneplus_zoom_catalog().back.clone().expect("задняя шкала OnePlus 11")
}

#[test]
fn default_plan_runs_taps_then_flip_round_trip_then_drag_and_ends_on_one_x() {
    let steps = planned_steps(&ZoomScenarioConfig::default());
    let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "01_tap_0.6",
            "02_tap_1",
            "03_tap_2",
            "04_tap_1",
            "05_tap_0.6",
            "06_flip_to_front",
            "07_flip_to_back",
            "08_tap_1_before_drag",
            "09_wheel_drag",
            "10_tap_1_before_seamless",
            "11_wheel_seamless_out",
            "12_wheel_seamless_back",
            "13_wheel_seamless_release",
            "14_pinch_seamless_out",
            "15_pinch_seamless_back",
            "16_pinch_seamless_release",
            "17_tap_1_final",
        ]
    );
    assert_eq!(
        steps[10].action,
        ZoomAction::Seamless {
            gesture: SeamlessGesture::Wheel,
            leg: SeamlessLeg::Out
        }
    );
    assert_eq!(
        steps[15].action,
        ZoomAction::Seamless {
            gesture: SeamlessGesture::Pinch,
            leg: SeamlessLeg::Release
        }
    );
    assert_eq!(steps[5].action, ZoomAction::Flip);
    assert_eq!(steps[8].action, ZoomAction::WheelDrag);
    assert_eq!(
        steps.last().map(|step| step.action),
        Some(ZoomAction::TapStop { ratio: 1.0 })
    );
}

#[test]
fn flip_and_drag_can_be_switched_off_but_the_final_one_x_tap_stays() {
    let config = ZoomScenarioConfig {
        stop_ratios: vec![2.0],
        flip_round_trip: false,
        wheel_drag: crate::zoom_scenario_config::WheelDragPlan {
            samples: 0,
            ..Default::default()
        },
        seamless: crate::zoom_scenario_config::SeamlessPlan {
            enabled: false,
            ..Default::default()
        },
        ..ZoomScenarioConfig::default()
    };
    let steps = planned_steps(&config);
    let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
    assert_eq!(names, vec!["01_tap_2", "02_tap_1_final"]);
}

#[test]
fn nearest_stop_finds_the_button_by_ratio_and_refuses_a_missing_one() {
    let group = back_group();
    let (ultra, ultra_kind) = nearest_stop(&group, 0.6).expect("кнопка .6 есть");
    assert_eq!(ultra_kind, ZoomStopKind::Lens(LensRole::UltraWide));
    assert!(group.stops[ultra].ratio.value() < 0.7, "кнопка .6 — это 0,59");
    let (_, one_kind) = nearest_stop(&group, 1.0).expect("кнопка 1 есть");
    assert_eq!(one_kind, ZoomStopKind::Lens(LensRole::Wide));
    let (_, two_kind) = nearest_stop(&group, 2.0).expect("кнопка 2 есть");
    assert_eq!(two_kind, ZoomStopKind::DigitalCrop);
    assert_eq!(nearest_stop(&group, 5.0), None, "кнопки 5× на ленте нет");
}

#[test]
fn a_tap_expects_a_switch_only_when_the_target_lens_is_not_already_active() {
    let digital = ZoomStopKind::DigitalCrop;
    let ultra = ZoomStopKind::Lens(LensRole::UltraWide);
    assert_eq!(
        expectation_for_tap(digital, Some(LensRole::Wide)),
        SwitchExpectation::NoSwitch
    );
    assert_eq!(
        expectation_for_tap(digital, Some(LensRole::UltraWide)),
        SwitchExpectation::ToRole(LensRole::Wide),
        "2× с ширика — это возврат на основную камеру"
    );
    assert_eq!(
        expectation_for_tap(ultra, Some(LensRole::Wide)),
        SwitchExpectation::ToRole(LensRole::UltraWide)
    );
    assert_eq!(
        expectation_for_tap(ultra, Some(LensRole::UltraWide)),
        SwitchExpectation::NoSwitch
    );
    assert_eq!(
        expectation_for_tap(ultra, None),
        SwitchExpectation::ToRole(LensRole::UltraWide)
    );
}

#[test]
fn expectations_accept_only_matching_completed_switches() {
    assert!(SwitchExpectation::ToRole(LensRole::Front).accepts(LensRole::Front));
    assert!(!SwitchExpectation::ToRole(LensRole::Front).accepts(LensRole::Wide));
    assert!(SwitchExpectation::ToAnyBack.accepts(LensRole::UltraWide));
    assert!(!SwitchExpectation::ToAnyBack.accepts(LensRole::Front));
    assert!(
        !SwitchExpectation::NoSwitch.accepts(LensRole::Wide),
        "смены не ждали — любая смена лишняя"
    );
}

#[test]
fn the_facing_after_a_step_follows_the_expectation() {
    assert_eq!(
        SwitchExpectation::ToRole(LensRole::Front).facing_after(Some(LensFacing::Back)),
        Some(LensFacing::Front)
    );
    assert_eq!(
        SwitchExpectation::ToAnyBack.facing_after(Some(LensFacing::Front)),
        Some(LensFacing::Back)
    );
    assert_eq!(
        SwitchExpectation::NoSwitch.facing_after(Some(LensFacing::Front)),
        Some(LensFacing::Front)
    );
}
