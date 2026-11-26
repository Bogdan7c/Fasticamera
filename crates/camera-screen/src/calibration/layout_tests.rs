//! Тесты геометрии раздела калибровки: зоны покрывают экран без щелей и перекрытий в обеих ориентациях, картинка
//! прибора равна зоне в пикселях, развёртка и поворот соответствуют положению телефона, перевод глаз ↔ поверхность.

use scope_contract::{MAX_SCOPE_IMAGE_SIDE_PX, ScopeKind, ScopePictureShape, ScopeScan};
use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use ui_core::geometry::{Point, Rect};

use super::{CalibrationConfig, CalibrationLayout, CalibrationLayoutError};
use crate::layout::{LayoutConfig, ScreenLayout};
use crate::mode_behaviour::LayoutPolicy;
use crate::test_support::OUR_SURFACE;
use crate::widgets_config::WidgetLayoutConfig;

const ALL_ORIENTATIONS: [DeviceOrientation; 4] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::PortraitUpsideDown,
    DeviceOrientation::LandscapeRight,
];

fn screen() -> ScreenLayout {
    ScreenLayout::new(&LayoutConfig::default(), OUR_SURFACE).expect("раскладка")
}

fn calibration_layout(policy: LayoutPolicy, orientation: DeviceOrientation) -> CalibrationLayout {
    screen()
        .calibration_layout(
            &CalibrationConfig::default(),
            &WidgetLayoutConfig::default().bottom,
            policy,
            orientation,
            SafeAreaInsets::NONE,
        )
        .expect("раскладка раздела")
}

/// Раскладка, следующая за телефоном, с отступами безопасной зоны.
fn calibration_layout_in(
    orientation: DeviceOrientation,
    safe_area: SafeAreaInsets,
) -> Result<CalibrationLayout, CalibrationLayoutError> {
    screen().calibration_layout(
        &CalibrationConfig::default(),
        &WidgetLayoutConfig::default().bottom,
        LayoutPolicy::FollowsOrientation,
        orientation,
        safe_area,
    )
}

fn area(rect: &Rect) -> f64 {
    rect.size.width * rect.size.height
}

fn overlap_area(first: &Rect, second: &Rect) -> f64 {
    let width = (first.max_x().min(second.max_x()) - first.min_x().max(second.min_x())).max(0.0);
    let height = (first.max_y().min(second.max_y()) - first.min_y().max(second.min_y())).max(0.0);
    width * height
}

#[test]
fn zones_tile_the_eye_rectangle_in_every_orientation() {
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let zones = layout.zones();
        let all = [zones.knobs, zones.clean_preview, zones.scope, zones.bottom_row];
        let eye = Rect::from_xywh(0.0, 0.0, layout.eye_size().width, layout.eye_size().height);

        let total: f64 = all.iter().map(area).sum();
        assert!(
            (total - area(&eye)).abs() < 1e-6,
            "{orientation:?}: площадь зон {total} против экрана {}",
            area(&eye)
        );
        for (index, first) in all.iter().enumerate() {
            assert!(
                first.min_x() >= -1e-9
                    && first.min_y() >= -1e-9
                    && first.max_x() <= eye.max_x() + 1e-9
                    && first.max_y() <= eye.max_y() + 1e-9,
                "{orientation:?}: зона {index} вышла за экран: {first:?}"
            );
            for second in &all[index + 1..] {
                assert!(
                    overlap_area(first, second) < 1e-9,
                    "{orientation:?}: зоны перекрываются: {first:?} / {second:?}"
                );
            }
        }
    }
}

#[test]
fn portrait_layout_follows_the_owner_sketch() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let zones = layout.zones();
    // Ручки слева на всю высоту над нижним рядом, справа сверху чистое превью, ниже прибор.
    assert_eq!(zones.knobs.min_x(), 0.0);
    assert_eq!(zones.knobs.min_y(), 0.0);
    assert_eq!(zones.knobs.max_y(), zones.bottom_row.min_y());
    assert_eq!(zones.clean_preview.min_x(), zones.knobs.max_x());
    assert_eq!(zones.clean_preview.max_y(), zones.scope.min_y());
    assert_eq!(zones.scope.max_y(), zones.bottom_row.min_y());
    assert_eq!(layout.rotation_clockwise_degrees(), 0.0);
}

#[test]
fn landscape_eye_is_the_surface_turned_and_knobs_take_a_third_of_the_width() {
    let surface = screen().screen_size();
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::LandscapeLeft);

    assert_eq!(layout.eye_size().width, surface.height);
    assert_eq!(layout.eye_size().height, surface.width);
    let knobs_share = layout.zones().knobs.size.width / layout.eye_size().width;
    assert!((knobs_share - 1.0 / 3.0).abs() < 1e-9, "колонка ручек {knobs_share}");
    assert_eq!(layout.rotation_clockwise_degrees(), 90.0);
}

#[test]
fn scope_image_size_equals_the_scope_zone_in_pixels_for_each_orientation() {
    let pixels_per_point = screen().point_scale().pixels_per_point();
    let mut sizes = Vec::new();
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let zone = layout.zones().scope;
        let size = layout.scope_image_size();
        assert_eq!(
            (size.width(), size.height()),
            (
                (zone.size.width * pixels_per_point).round() as u32,
                (zone.size.height * pixels_per_point).round() as u32
            ),
            "{orientation:?}"
        );
        assert!(size.width() >= 1 && size.height() >= 1);
        assert!(size.width() <= MAX_SCOPE_IMAGE_SIDE_PX && size.height() <= MAX_SCOPE_IMAGE_SIDE_PX);
        sizes.push((size.width(), size.height()));
    }
    // Портрет и альбом дают разные размеры (зоны разные), а положение вверх ногами повторяет портрет.
    assert_ne!(sizes[0], sizes[1]);
    assert_eq!(sizes[0], sizes[2]);
    assert_eq!(sizes[1], sizes[3]);
}

#[test]
fn each_scope_kind_gets_the_picture_size_of_its_shape_zone_or_inscribed_square() {
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let zone = layout.scope_image_size();
        for kind in ScopeKind::ALL {
            let size = layout.scope_image_size_for(kind);
            match kind.picture_shape() {
                // Waveform и parade занимают всю зону.
                ScopePictureShape::FillsZone => assert_eq!(size, zone, "{kind:?} {orientation:?}"),
                // Вектороскоп — квадрат с меньшей стороной зоны: круг не превращается в овал.
                ScopePictureShape::Square => {
                    let side = zone.width().min(zone.height());
                    assert_eq!((size.width(), size.height()), (side, side), "{kind:?} {orientation:?}");
                    assert_ne!(
                        (size.width(), size.height()),
                        (zone.width(), zone.height()),
                        "зона не квадратная: квадрат отличается от неё"
                    );
                }
            }
        }
    }
}

#[test]
fn square_picture_respects_the_side_limit_of_the_zone_picture() {
    let config = CalibrationConfig {
        max_scope_image_side_px: 100,
        ..CalibrationConfig::default()
    };
    let layout = screen()
        .calibration_layout(
            &config,
            &WidgetLayoutConfig::default().bottom,
            LayoutPolicy::FollowsOrientation,
            DeviceOrientation::Portrait,
            SafeAreaInsets::NONE,
        )
        .expect("раскладка");
    let square = layout.scope_image_size_for(ScopeKind::Vectorscope);
    assert_eq!((square.width(), square.height()), (100, 100));
}

#[test]
fn scope_size_is_clamped_to_the_configured_side_limit() {
    let config = CalibrationConfig {
        max_scope_image_side_px: 100,
        ..CalibrationConfig::default()
    };
    let layout = screen()
        .calibration_layout(
            &config,
            &WidgetLayoutConfig::default().bottom,
            LayoutPolicy::FollowsOrientation,
            DeviceOrientation::Portrait,
            SafeAreaInsets::NONE,
        )
        .expect("раскладка");

    assert!(layout.scope_image_size().width() <= 100 && layout.scope_image_size().height() <= 100);
    assert_eq!(
        layout.scope_image_size().height(),
        100,
        "высота зоны больше предела и зажата по нему"
    );
}

#[test]
fn scan_matches_the_phone_position_table_of_the_scope_contract() {
    let expected = [
        (DeviceOrientation::Portrait, ScopeScan::Upright),
        (DeviceOrientation::LandscapeLeft, ScopeScan::RotatedClockwise),
        (DeviceOrientation::PortraitUpsideDown, ScopeScan::UpsideDown),
        (DeviceOrientation::LandscapeRight, ScopeScan::RotatedCounterClockwise),
    ];
    for (orientation, scan) in expected {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        assert_eq!(layout.scope_scan(), scan, "{orientation:?}");
    }
}

#[test]
fn a_portrait_fixed_policy_ignores_the_phone_position() {
    let fixed = calibration_layout(LayoutPolicy::PortraitFixed, DeviceOrientation::LandscapeLeft);
    let portrait = calibration_layout(LayoutPolicy::PortraitFixed, DeviceOrientation::Portrait);

    assert_eq!(fixed, portrait);
    assert_eq!(fixed.scope_scan(), ScopeScan::Upright);
    assert_eq!(fixed.rotation_clockwise_degrees(), 0.0);
}

#[test]
fn eye_and_surface_points_round_trip_in_every_orientation() {
    let probes = [Point::new(0.0, 0.0), Point::new(123.4, 56.7), Point::new(300.0, 200.0)];
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        for probe in probes {
            let back = layout.surface_point_in_eye(layout.eye_point_on_surface(probe));
            assert!(
                (back.x - probe.x).abs() < 1e-9 && (back.y - probe.y).abs() < 1e-9,
                "{orientation:?}: {probe:?} → {back:?}"
            );
        }
    }
}

#[test]
fn eye_corners_land_on_the_corners_the_icon_rotation_implies() {
    let surface = screen().screen_size();
    // Телефон LandscapeLeft (верх смотрит влево): левый верхний угол «глаза» — правый верхний угол поверхности.
    let left = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::LandscapeLeft);
    let corner = left.eye_point_on_surface(Point::new(0.0, 0.0));
    assert!(
        (corner.x - surface.width).abs() < 1e-9 && corner.y.abs() < 1e-9,
        "{corner:?}"
    );
    // LandscapeRight — левый нижний.
    let right = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::LandscapeRight);
    let corner = right.eye_point_on_surface(Point::new(0.0, 0.0));
    assert!(
        corner.x.abs() < 1e-9 && (corner.y - surface.height).abs() < 1e-9,
        "{corner:?}"
    );
    // Вверх ногами — правый нижний.
    let flipped = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::PortraitUpsideDown);
    let corner = flipped.eye_point_on_surface(Point::new(0.0, 0.0));
    assert!((corner.x - surface.width).abs() < 1e-9 && (corner.y - surface.height).abs() < 1e-9);
}

#[test]
fn zones_mapped_to_the_surface_still_tile_the_whole_surface() {
    let surface_area = {
        let size = screen().screen_size();
        size.width * size.height
    };
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let zones = layout.zones();
        let mapped: Vec<Rect> = [zones.knobs, zones.clean_preview, zones.scope, zones.bottom_row]
            .iter()
            .map(|zone| layout.eye_rect_on_surface(*zone))
            .collect();

        let total: f64 = mapped.iter().map(area).sum();
        assert!(
            (total - surface_area).abs() < 1e-6,
            "{orientation:?}: {total} против {surface_area}"
        );
        for (index, first) in mapped.iter().enumerate() {
            for second in &mapped[index + 1..] {
                assert!(
                    overlap_area(first, second) < 1e-9,
                    "{orientation:?}: {first:?} / {second:?}"
                );
            }
        }
    }
}

#[test]
fn menu_button_and_carousel_sit_inside_the_bottom_row_and_menu_stays_on_the_right() {
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let row = layout.zones().bottom_row;
        let radius = WidgetLayoutConfig::default().bottom.menu_button.diameter_pt / 2.0;
        let menu = layout.menu_button_center();
        let carousel = layout.carousel_center();

        assert!(
            menu.y - radius >= row.min_y() && menu.y + radius <= row.max_y(),
            "{orientation:?}: кнопка меню вне нижнего ряда: {menu:?} / {row:?}"
        );
        assert!(
            row.contains(carousel),
            "{orientation:?}: капсула вне нижнего ряда: {carousel:?} / {row:?}"
        );
        assert!(menu.x > carousel.x, "{orientation:?}: меню справа от капсулы");
    }
}

#[test]
fn portrait_menu_button_stays_where_photo_puts_it() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let photo = WidgetLayoutConfig::default().bottom;
    let shift = screen().bottom_anchor_shift_pt();

    assert_eq!(layout.menu_button_center().x, photo.menu_button.center_x_pt);
    assert_eq!(layout.menu_button_center().y, photo.menu_button.center_y_pt + shift);
}

#[test]
fn zone_edges_sit_on_whole_pixels_so_the_scope_image_maps_one_to_one() {
    let pixels_per_point = screen().point_scale().pixels_per_point();
    let on_grid = |value_pt: f64| ((value_pt * pixels_per_point) - (value_pt * pixels_per_point).round()).abs() < 1e-6;
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let scope = layout.zones().scope;
        for edge in [scope.min_x(), scope.max_x(), scope.min_y(), scope.max_y()] {
            assert!(on_grid(edge), "{orientation:?}: край прибора {edge} pt не на пикселе");
        }
        assert_eq!(
            f64::from(layout.scope_image_size().width()),
            scope.size.width * pixels_per_point,
            "{orientation:?}: ширина картинки в точности ширина зоны"
        );
    }
}

// ----- безопасная зона (14k) -----------------------------------------------------------------------------------------

/// Отступы с разным числом у каждого края: любая путаница краёв видна.
fn distinct_insets() -> SafeAreaInsets {
    SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 10,
        top: 20,
        right: 30,
        bottom: 40,
    })
    .expect("отступы")
}

#[test]
fn surface_insets_turn_into_eye_edges_with_the_same_rotation_as_the_layout() {
    // Слева, сверху и справа глазами — какой край поверхности это: портрет, левый бок, вверх ногами, правый бок.
    let expected_px = [
        (DeviceOrientation::Portrait, (10.0, 20.0, 30.0)),
        (DeviceOrientation::LandscapeLeft, (20.0, 30.0, 40.0)),
        (DeviceOrientation::PortraitUpsideDown, (30.0, 40.0, 10.0)),
        (DeviceOrientation::LandscapeRight, (40.0, 10.0, 20.0)),
    ];
    let scale = screen().point_scale().pixels_per_point();
    for (orientation, (left_px, top_px, right_px)) in expected_px {
        let plain = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let layout = calibration_layout_in(orientation, distinct_insets()).expect("раскладка");
        let (knobs, scope) = (layout.zones().knobs, layout.zones().scope);
        let close = |actual: f64, expected: f64| (actual - expected).abs() < 1e-6;
        assert!(close(knobs.min_x() * scale, left_px), "{orientation:?}: {knobs:?}");
        assert!(close(knobs.min_y() * scale, top_px), "{orientation:?}: {knobs:?}");
        assert!(
            close((layout.eye_size().width - scope.max_x()) * scale, right_px),
            "{orientation:?}: {scope:?}"
        );
        // Колонка сдвигается, а не сужается; низ и линия «превью / прибор» на месте; зоны встык.
        assert!(
            close(knobs.size.width, plain.zones().knobs.size.width),
            "{orientation:?}"
        );
        assert!(close(knobs.max_y(), plain.zones().knobs.max_y()), "{orientation:?}");
        assert!(close(scope.min_y(), plain.zones().scope.min_y()), "{orientation:?}");
        assert!(close(scope.min_x(), knobs.max_x()), "{orientation:?}");
        // Чистое превью — картинка: до правого и верхнего краёв экрана.
        let clean = layout.zones().clean_preview;
        assert!(close(clean.max_x(), layout.eye_size().width) && close(clean.min_y(), 0.0));
        assert_eq!(
            (layout.scope_image_size().width(), layout.scope_image_size().height()),
            (
                (scope.size.width * scale).round() as u32,
                (scope.size.height * scale).round() as u32
            ),
            "{orientation:?}: картинка прибора ровно с его зону"
        );
    }
}

#[test]
fn portrait_bottom_row_keeps_the_photo_places_and_the_landscape_menu_button_moves_by_the_inset() {
    let plain = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let safe = calibration_layout_in(DeviceOrientation::Portrait, distinct_insets()).expect("раскладка");
    assert_eq!(safe.menu_button_center(), plain.menu_button_center());
    assert_eq!(safe.carousel_center(), plain.carousel_center());

    let scale = screen().point_scale().pixels_per_point();
    let plain = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::LandscapeLeft);
    let safe = calibration_layout_in(DeviceOrientation::LandscapeLeft, distinct_insets()).expect("раскладка");
    let moved_px = (plain.menu_button_center().x - safe.menu_button_center().x) * scale;
    assert!((moved_px - 40.0).abs() < 1e-6, "сдвиг {moved_px}");
    assert_eq!(safe.carousel_center(), plain.carousel_center());
}

#[test]
fn insets_that_leave_no_room_are_an_error_not_an_inverted_zone() {
    for raw in [
        RawEdgeInsetsPx {
            left: 0,
            top: 3000,
            right: 0,
            bottom: 0,
        },
        RawEdgeInsetsPx {
            left: 0,
            top: 0,
            right: 700,
            bottom: 0,
        },
        RawEdgeInsetsPx {
            left: 0,
            top: 0,
            right: 0,
            bottom: 3000,
        },
    ] {
        let insets = SafeAreaInsets::try_from_insets(raw).expect("отступы");
        assert!(
            matches!(
                calibration_layout_in(DeviceOrientation::Portrait, insets),
                Err(CalibrationLayoutError::NoRoomInSafeArea { .. })
            ),
            "{raw:?}"
        );
    }
}
