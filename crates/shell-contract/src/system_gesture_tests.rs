use super::*;

/// Экран телефона владельца и зоны, которые он сообщил (снизу 132 px, сверху 156 px).
const SURFACE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};

fn owner_zones() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 156,
        right: 0,
        bottom: 132,
    })
    .expect("зоны владельца корректны")
}

fn at(x_px: f32, y_px: f32) -> SurfacePoint {
    SurfacePoint { x_px, y_px }
}

#[test]
fn valid_insets_keep_each_edge_in_its_own_slot() {
    let zones = SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 1,
        top: 2,
        right: 3,
        bottom: 4,
    })
    .expect("корректные толщины");
    assert_eq!(zones.thickness_px(ScreenEdge::Left), 1);
    assert_eq!(zones.thickness_px(ScreenEdge::Top), 2);
    assert_eq!(zones.thickness_px(ScreenEdge::Right), 3);
    assert_eq!(zones.thickness_px(ScreenEdge::Bottom), 4);
}

#[test]
fn negative_thickness_is_rejected_with_the_offending_edge() {
    let error = SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 0,
        right: -5,
        bottom: 0,
    })
    .expect_err("отрицательная толщина");
    assert_eq!(
        error,
        EdgeInsetsError::NegativeThickness {
            edge: ScreenEdge::Right,
            thickness_px: -5
        }
    );
}

#[test]
fn default_is_none_and_contains_nothing() {
    let zones = SystemGestureZones::default();
    assert!(zones.is_none());
    for point in [at(0.0, 0.0), at(1079.0, 2411.0), at(540.0, 2400.0)] {
        assert!(!zones.contains(SURFACE, point), "{point:?}");
    }
    assert!(!owner_zones().is_none());
}

#[test]
fn bottom_and_top_bands_use_half_open_boundaries() {
    let zones = owner_zones();
    // Нижняя полоса [2280, 2412): первая строка полосы — в зоне, строка выше — нет.
    assert!(zones.contains(SURFACE, at(540.0, 2280.0)));
    assert!(zones.contains(SURFACE, at(540.0, 2411.0)));
    assert!(!zones.contains(SURFACE, at(540.0, 2279.9)));
    // Верхняя полоса [0, 156).
    assert!(zones.contains(SURFACE, at(540.0, 0.0)));
    assert!(zones.contains(SURFACE, at(540.0, 155.9)));
    assert!(!zones.contains(SURFACE, at(540.0, 156.0)));
}

#[test]
fn side_bands_work_and_middle_of_screen_is_free() {
    let zones = SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 20,
        top: 0,
        right: 30,
        bottom: 0,
    })
    .expect("корректные толщины");
    assert!(zones.contains(SURFACE, at(19.9, 1000.0)));
    assert!(!zones.contains(SURFACE, at(20.0, 1000.0)));
    assert!(zones.contains(SURFACE, at(1050.0, 1000.0)));
    assert!(!zones.contains(SURFACE, at(1049.9, 1000.0)));
    assert!(!owner_zones().contains(SURFACE, at(540.0, 1200.0)));
}

#[test]
fn nan_coordinates_do_not_belong_to_the_system() {
    assert!(!owner_zones().contains(SURFACE, at(f32::NAN, f32::NAN)));
}
