//! Правило «кого греть» на синтетической шкале из трёх объективов (ширик / основная / телевик).

use camera_contract::catalog::CameraCatalog;
use camera_contract::fixtures::{camera_id, main_wide_camera, oneplus11_visible_cameras};
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::{ZoomCatalog, ZoomRatio};

use super::{ZoomConfig, build_zoom_catalog, standby_candidates};
use crate::config::RoleClassifierConfig;
use crate::role_classifier::classify_catalog;

fn three_lens_scale() -> ZoomCatalog {
    let mut tele = main_wide_camera();
    tele.id = camera_id("9");
    tele.capabilities.optics.focal_lengths_mm = vec![7.08];
    tele.capabilities.optics.sensor_physical_size.width_mm = 5.248;
    tele.capabilities.optics.sensor_physical_size.height_mm = 3.942;
    let mut cameras = oneplus11_visible_cameras();
    cameras.push(tele);
    let catalog: CameraCatalog = classify_catalog(cameras, &RoleClassifierConfig::default())
        .expect("образцы классифицируются")
        .catalog;
    build_zoom_catalog(&catalog, &ZoomConfig::default())
}

fn roles(scale: &ZoomCatalog, current: LensRole, ratio: f32, capacity: usize) -> Vec<LensRole> {
    standby_candidates(
        scale,
        LensFacing::Back,
        current,
        ZoomRatio::new(ratio).expect("коэффициент"),
        capacity,
    )
    .into_iter()
    .map(|lens| lens.role)
    .collect()
}

#[test]
fn capacity_one_picks_the_neighbour_closest_to_the_current_ratio() {
    let scale = three_lens_scale();
    // Основная на 1,0×: ширик (0,59×) ближе телевика (≈2×).
    assert_eq!(roles(&scale, LensRole::Wide, 1.0, 1), vec![LensRole::UltraWide]);
    // Колесо уехало к 1,8×: ближе телевик.
    assert_eq!(roles(&scale, LensRole::Wide, 1.8, 1), vec![LensRole::Tele]);
}

#[test]
fn capacity_two_returns_both_neighbours_closest_first() {
    let scale = three_lens_scale();
    assert_eq!(
        roles(&scale, LensRole::Wide, 1.8, 2),
        vec![LensRole::Tele, LensRole::UltraWide]
    );
    assert_eq!(
        roles(&scale, LensRole::Wide, 0.7, 2),
        vec![LensRole::UltraWide, LensRole::Tele]
    );
    // Ёмкость больше числа соседей — отдаём всех соседей и не больше.
    assert_eq!(roles(&scale, LensRole::Wide, 1.0, 5).len(), 2);
}

#[test]
fn zero_capacity_unknown_lens_and_single_lens_groups_have_no_candidates() {
    let scale = three_lens_scale();
    assert!(roles(&scale, LensRole::Wide, 1.0, 0).is_empty());
    // Фронтальная группа из одного объектива: соседей нет.
    let front = standby_candidates(&scale, LensFacing::Front, LensRole::Front, ZoomRatio::ONE, 3);
    assert!(front.is_empty());
    // Объектива направления `Front` в задней группе нет.
    assert!(roles(&scale, LensRole::Front, 1.0, 2).is_empty());
    // Направление без шкалы.
    assert!(
        standby_candidates(
            &ZoomCatalog::default(),
            LensFacing::Back,
            LensRole::Wide,
            ZoomRatio::ONE,
            2
        )
        .is_empty()
    );
}
