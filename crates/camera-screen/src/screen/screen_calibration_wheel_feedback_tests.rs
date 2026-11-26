//! Числа толчка колеса и щелчки выбора (решение 286): что видит и чувствует человек в строке названия колеса,
//! на вкладках колёс и на точках полос.

use look_contract::{ColorWheel, WheelPush, WheelZone};
use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;
use ui_kit::tokens::{HapticEvent, TextRoleKind, UiKitTokens};

use super::screen_calibration_bands_tests::scroll_list_to_end;
use super::screen_calibration_tests::{center, portrait};
use super::screen_calibration_wheels_tests::{facts_with_wheel, wheels};
use super::test_rig::*;
use crate::effect::Effect;
use crate::look_test_support::look_facts;

/// Паттерн щелчка выбора из таблицы токенов (то, что обязан сыграть экран).
fn selection_pattern() -> HapticPattern {
    UiKitTokens::default()
        .haptics
        .pattern_for(HapticEvent::CarouselSelection)
        .expect("щелчок выбора включён токенами")
}

fn haptics_in(effects: &[Effect]) -> Vec<HapticPattern> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Haptic(pattern) => Some(*pattern),
            _ => None,
        })
        .collect()
}

fn pushed_lift() -> look_contract::LookFacts {
    let push = WheelPush::from_hue_degrees(210.0, 0.35).expect("толчок внутри круга");
    facts_with_wheel(
        WheelZone::Lift,
        ColorWheel {
            push,
            ..ColorWheel::NEUTRAL
        },
    )
}

#[test]
fn the_title_row_shows_the_push_angle_and_strength_of_the_selected_wheel() {
    let mut rig = wheels(pushed_lift(), DeviceOrientation::Portrait);
    let list = rig.frame().clone();
    assert_eq!(
        texts_of(&list, "210° · 35", TextRoleKind::ScaleValue).len(),
        1,
        "у выбранного LIFT в строке названия угол и сила толчка"
    );
    // Другое колесо с нейтральным толчком: чисел нет, вместо них длинное тире; чужие числа не показываются.
    let gamma_tab = center(&rig, "calib_wheel_tab", 1);
    rig.tap(gamma_tab);
    rig.run(300);
    let list = rig.frame().clone();
    assert_eq!(texts_of(&list, "210° · 35", TextRoleKind::ScaleValue).len(), 0);
    assert_eq!(
        texts_of(&list, "—", TextRoleKind::ScaleValue).len(),
        1,
        "нейтральный толчок — тире"
    );
}

#[test]
fn a_tap_on_a_wheel_tab_clicks_once_and_a_tap_on_the_selected_tab_is_silent() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let gamma_tab = center(&rig, "calib_wheel_tab", 1);
    rig.tap(gamma_tab);
    rig.run(200);
    assert_eq!(
        haptics_in(&rig.effects()),
        vec![selection_pattern()],
        "смена вкладки — один лёгкий щелчок"
    );
    rig.tap(gamma_tab);
    rig.run(200);
    assert!(
        haptics_in(&rig.effects()).is_empty(),
        "тап по уже выбранной вкладке молчит"
    );
}

#[test]
fn a_tap_on_a_band_dot_clicks_once_and_a_tap_on_the_selected_dot_is_silent() {
    let mut rig = portrait(look_facts());
    scroll_list_to_end(&mut rig);
    rig.effects();
    let sky = center(&rig, "calib_band_dot", 1);
    rig.tap(sky);
    rig.run(200);
    assert_eq!(
        haptics_in(&rig.effects()),
        vec![selection_pattern()],
        "смена полосы — один лёгкий щелчок"
    );
    rig.tap(sky);
    rig.run(200);
    assert!(haptics_in(&rig.effects()).is_empty(), "тап по выбранной точке молчит");
}
