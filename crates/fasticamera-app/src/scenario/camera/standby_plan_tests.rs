//! Правила `standby`: «дом» по шкале, коэффициенты плавных переходов с учётом гистерезиса, частоты по окнам.

use camera_contract::ids::LensRole;

use super::*;
use crate::camera_test_support::oneplus_zoom_catalog;

fn back_group() -> LensGroup {
    oneplus_zoom_catalog().back.clone().expect("задняя шкала OnePlus 11")
}

fn lens(group: &LensGroup, role: LensRole) -> LensZoomPoint {
    group.lens_for_role(role).expect("объектив в шкале").clone()
}

#[test]
fn home_is_the_reference_lens_closest_to_one_x() {
    let group = back_group();
    assert_eq!(home_lens(&group).map(|home| home.role), Some(LensRole::Wide));
    assert!(back_home(&oneplus_zoom_catalog()).is_some());
}

#[test]
fn a_catalog_without_a_back_group_has_no_home() {
    assert!(back_home(&ZoomCatalog::default()).is_none());
}

#[test]
fn going_down_the_scale_uses_the_neighbours_own_ratio() {
    let group = back_group();
    let ratio = ratio_for_continuous_switch(
        &group,
        &lens(&group, LensRole::Wide),
        &lens(&group, LensRole::UltraWide),
        1.10,
    );
    let ultra = lens(&group, LensRole::UltraWide).native_ratio.value();
    assert!((ratio.value() - ultra).abs() < 1e-6, "{ratio:?} против {ultra}");
}

#[test]
fn going_up_the_scale_overshoots_the_hysteresis_band() {
    let group = back_group();
    let ratio = ratio_for_continuous_switch(
        &group,
        &lens(&group, LensRole::UltraWide),
        &lens(&group, LensRole::Wide),
        1.10,
    );
    let wide = lens(&group, LensRole::Wide).native_ratio.value();
    assert!(
        ratio.value() >= wide * 1.05,
        "ниже порога селектора (native × 1,05) переключение вверх не случится: {ratio:?}"
    );
}

#[test]
fn the_result_never_leaves_the_group_range() {
    let group = back_group();
    let wide = lens(&group, LensRole::Wide);
    let ratio = ratio_for_continuous_switch(&group, &lens(&group, LensRole::UltraWide), &wide, 1_000.0);
    assert!(ratio.value() <= group.range.max.value());
}

fn counters(published: u64, standby_returned: u64) -> RawStreamCounters {
    RawStreamCounters {
        frames_published: published,
        standby_frames_returned: standby_returned,
        ..RawStreamCounters::default()
    }
}

#[test]
fn window_fps_counts_each_epoch_kind_separately() {
    let fps = window_fps(&counters(100, 10), &counters(220, 128), Duration::from_secs(2)).expect("окно не нулевое");
    assert!((fps.primary - 60.0).abs() < 1e-9);
    assert!((fps.standby - 59.0).abs() < 1e-9);
}

#[test]
fn zero_window_has_no_fps_and_a_counter_going_backwards_gives_zero() {
    assert_eq!(window_fps(&counters(0, 0), &counters(10, 10), Duration::ZERO), None);
    let fps = window_fps(&counters(50, 50), &counters(10, 10), Duration::from_secs(1)).expect("окно не нулевое");
    assert_eq!((fps.primary, fps.standby), (0.0, 0.0));
}
