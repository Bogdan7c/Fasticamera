//! Выбор камеры с гистерезисом на группах OnePlus 11 и синтетическом телевике.

use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras};
use camera_contract::ids::LensRole;
use camera_contract::zoom::{LensGroup, ZoomGesture, ZoomRatio};

use super::{ZoomConfig, build_zoom_catalog, select_initial_lens, select_lens};
use crate::config::RoleClassifierConfig;
use crate::role_classifier::classify_catalog;

fn ratio(value: f32) -> ZoomRatio {
    ZoomRatio::new(value).expect("валидный коэффициент")
}

fn back_group() -> LensGroup {
    let catalog = classify_catalog(oneplus11_visible_cameras(), &RoleClassifierConfig::default())
        .expect("образцы")
        .catalog;
    build_zoom_catalog(&catalog, &ZoomConfig::default())
        .back
        .expect("задняя группа")
}

fn three_lens_group() -> LensGroup {
    let mut tele = main_wide_camera();
    tele.id = camera_id("9");
    tele.capabilities.optics.focal_lengths_mm = vec![7.08];
    tele.capabilities.optics.sensor_physical_size.width_mm = 5.248;
    tele.capabilities.optics.sensor_physical_size.height_mm = 3.942;
    let mut cameras = oneplus11_visible_cameras();
    cameras.push(tele);
    let catalog = classify_catalog(cameras, &RoleClassifierConfig::default())
        .expect("образцы")
        .catalog;
    build_zoom_catalog(&catalog, &ZoomConfig::default())
        .back
        .expect("задняя группа")
}

/// Плавный жест (колесо, щипок): с гистерезисом.
fn pick(group: &LensGroup, value: f32, current: LensRole) -> LensRole {
    pick_with(group, value, current, ZoomGesture::Continuous)
}

/// Явный выбор (тап по кнопке): без гистерезиса.
fn pick_discrete(group: &LensGroup, value: f32, current: LensRole) -> LensRole {
    pick_with(group, value, current, ZoomGesture::Discrete)
}

fn pick_with(group: &LensGroup, value: f32, current: LensRole, gesture: ZoomGesture) -> LensRole {
    select_lens(group, ratio(value), current, gesture, &ZoomConfig::default()).expect("в группе есть камеры")
}

#[test]
fn oscillation_around_one_x_does_not_flip_flop_from_ultra_wide() {
    let group = back_group();
    let mut current = LensRole::UltraWide;
    for value in [0.98, 1.03, 0.98, 1.04, 0.99, 1.03] {
        current = pick(&group, value, current);
        assert_eq!(current, LensRole::UltraWide, "на {value} ширик держится");
    }
}

#[test]
fn going_up_from_ultra_wide_switches_at_the_hysteresis_point() {
    let group = back_group();
    assert_eq!(pick(&group, 1.04, LensRole::UltraWide), LensRole::UltraWide);
    assert_eq!(pick(&group, 1.05, LensRole::UltraWide), LensRole::Wide);
    assert_eq!(pick(&group, 3.0, LensRole::UltraWide), LensRole::Wide);
}

#[test]
fn going_down_from_wide_switches_immediately_below_one_x() {
    let group = back_group();
    assert_eq!(pick(&group, 1.0, LensRole::Wide), LensRole::Wide);
    assert_eq!(pick(&group, 0.97, LensRole::Wide), LensRole::UltraWide);
    assert_eq!(pick(&group, 0.6, LensRole::Wide), LensRole::UltraWide);
}

#[test]
fn single_lens_group_always_selects_that_lens() {
    let catalog = classify_catalog(vec![main_wide_camera()], &RoleClassifierConfig::default())
        .expect("образцы")
        .catalog;
    let group = build_zoom_catalog(&catalog, &ZoomConfig::default())
        .back
        .expect("группа");
    for value in [0.1, 1.0, 7.0] {
        assert_eq!(pick(&group, value, LensRole::Wide), LensRole::Wide);
        assert_eq!(select_initial_lens(&group, ratio(value)), Some(LensRole::Wide));
    }
}

#[test]
fn tele_group_behaves_the_same_way_with_hysteresis_upwards_and_immediate_downwards() {
    let group = three_lens_group();
    let tele_native = group
        .lens_for_role(LensRole::Tele)
        .expect("телевик")
        .native_ratio
        .value();
    let switch_point = tele_native * 1.05;
    // Едем вверх от основной: ниже порога держим основную, на пороге — телевик.
    assert_eq!(pick(&group, switch_point * 0.99, LensRole::Wide), LensRole::Wide);
    assert_eq!(pick(&group, switch_point * 1.001, LensRole::Wide), LensRole::Tele);
    // Едем вниз с телевика: ниже его собственного коэффициента — немедленно основная.
    assert_eq!(pick(&group, tele_native * 1.01, LensRole::Tele), LensRole::Tele);
    assert_eq!(pick(&group, tele_native * 0.99, LensRole::Tele), LensRole::Wide);
    // Скачок через две камеры: с ширика сразу к телевику.
    assert_eq!(pick(&group, 3.0, LensRole::UltraWide), LensRole::Tele);
}

#[test]
fn current_lens_from_another_group_falls_back_to_the_candidate() {
    let group = back_group();
    assert_eq!(pick(&group, 1.02, LensRole::Front), LensRole::Wide);
}

#[test]
fn initial_selection_has_no_hysteresis() {
    let group = back_group();
    assert_eq!(select_initial_lens(&group, ratio(1.02)), Some(LensRole::Wide));
    assert_eq!(select_initial_lens(&group, ratio(0.6)), Some(LensRole::UltraWide));
}

#[test]
fn discrete_tap_on_one_x_from_ultra_wide_selects_wide_while_continuous_stays() {
    let group = back_group();
    assert_eq!(pick_discrete(&group, 1.0, LensRole::UltraWide), LensRole::Wide);
    assert_eq!(pick(&group, 1.0, LensRole::UltraWide), LensRole::UltraWide);
    assert_eq!(pick(&group, 1.06, LensRole::UltraWide), LensRole::Wide);
}

#[test]
fn discrete_selection_still_goes_down_and_to_the_longest_lens_directly() {
    let group = three_lens_group();
    let tele_native = group
        .lens_for_role(LensRole::Tele)
        .expect("телевик")
        .native_ratio
        .value();
    assert_eq!(pick_discrete(&group, 0.6, LensRole::Wide), LensRole::UltraWide);
    assert_eq!(pick_discrete(&group, tele_native, LensRole::UltraWide), LensRole::Tele);
    assert_eq!(pick_discrete(&group, tele_native, LensRole::Wide), LensRole::Tele);
}

#[test]
fn hysteresis_does_not_skip_the_intermediate_lens_when_the_longest_threshold_is_not_reached() {
    let group = three_lens_group();
    let tele_native = group
        .lens_for_role(LensRole::Tele)
        .expect("телевик")
        .native_ratio
        .value();
    // Кандидат — телевик, но его порог (native * 1,05) ещё не пройден; порог основной пройден давно.
    let between = tele_native * 1.02;
    assert_eq!(pick(&group, between, LensRole::UltraWide), LensRole::Wide);
    // Порог телевика пройден — уже телевик.
    assert_eq!(pick(&group, tele_native * 1.06, LensRole::UltraWide), LensRole::Tele);
    // Нет ни одного пройденного порога — остаёмся.
    assert_eq!(pick(&group, 1.04, LensRole::UltraWide), LensRole::UltraWide);
}
