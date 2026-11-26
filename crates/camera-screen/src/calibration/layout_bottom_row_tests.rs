//! Тесты нижнего ряда раздела 14n: капсула, кнопка меню справа и CLEAN слева стоят на одной линии со своими числами
//! раздела (решения 256, 259), внутри безопасной зоны владельца.

use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use ui_core::geometry::{Rect, Size};

use super::layout_tests::{ALL_ORIENTATIONS, calibration_layout, calibration_layout_in, distinct_insets, screen};
use super::{CalibrationConfig, CapsuleLine};
use crate::config::ScreenConfig;
use crate::mode_behaviour::LayoutPolicy;

/// Полосы телефона владельца, px: верх 156, низ 132.
fn owner_insets() -> SafeAreaInsets {
    SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 156,
        right: 0,
        bottom: 132,
    })
    .expect("отступы владельца")
}

#[test]
fn the_menu_button_the_capsule_and_clean_stand_on_one_line_in_every_orientation() {
    let inset = CalibrationConfig::default().layout.side_button_inset_pt;
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let (menu, clean, capsule) = (
            layout.menu_button_center(),
            layout.clean_button_center(),
            layout.carousel_center(),
        );
        let eye = layout.eye_size();
        assert_eq!(menu.y, capsule.y, "{orientation:?}: кнопка меню на линии капсулы");
        assert_eq!(clean.y, capsule.y, "{orientation:?}: CLEAN на линии капсулы");
        assert!(
            (menu.x - (eye.width - inset)).abs() < 1e-9,
            "{orientation:?}: меню у правого края"
        );
        assert!(
            (clean.x - inset).abs() < 1e-9,
            "{orientation:?}: CLEAN у левого края — зеркально меню"
        );
        assert!(
            (capsule.x - eye.width / 2.0).abs() < 1e-9,
            "{orientation:?}: капсула по центру"
        );
    }
}

#[test]
fn portrait_capsule_stays_where_photo_puts_it_by_the_sections_own_number() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let photo = ScreenConfig::default().widgets.bottom;
    let shift = screen().bottom_anchor_shift_pt();
    // Совпадение с PHOTO — свойство чисел, а не связь: у раздела своё поле конфига, и меняться они могут врозь.
    assert_eq!(layout.carousel_center().x, photo.capsule_center_x_pt);
    assert_eq!(layout.carousel_center().y, photo.capsule_center_y_pt + shift);
    let CapsuleLine::ReferenceAnchoredToBottom { reference_center_y_pt } =
        CalibrationConfig::default().layout.portrait.capsule_line
    else {
        panic!("в портрете линия капсулы привязана к низу, как у PHOTO");
    };
    assert_eq!(reference_center_y_pt, photo.capsule_center_y_pt);
}

#[test]
fn changing_the_section_capsule_number_moves_the_portrait_row_without_touching_photo() {
    let mut config = CalibrationConfig::default();
    config.layout.portrait.capsule_line = CapsuleLine::ReferenceAnchoredToBottom {
        reference_center_y_pt: 800.0,
    };
    let moved =
        super::layout_tests::calibration_layout_with(&config, DeviceOrientation::Portrait, SafeAreaInsets::NONE)
            .expect("раскладка");
    let shift = screen().bottom_anchor_shift_pt();
    for point in [
        moved.menu_button_center(),
        moved.clean_button_center(),
        moved.carousel_center(),
    ] {
        assert_eq!(point.y, 800.0 + shift, "вся строка идёт за собственным числом раздела");
    }
}

#[test]
fn landscape_line_is_the_middle_of_the_bottom_row() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let row = layout.zones().bottom_row;
        assert_eq!(layout.carousel_center().y, row.center().y, "{orientation:?}");
        let radius = ScreenConfig::default().widgets.bottom.menu_button.diameter_pt / 2.0;
        let (menu, clean) = (layout.menu_button_center(), layout.clean_button_center());
        for button in [menu, clean] {
            assert!(
                button.y - radius >= row.min_y() && button.y + radius <= row.max_y(),
                "{orientation:?}: кнопка в нижнем ряду целиком"
            );
        }
    }
}

#[test]
fn side_buttons_follow_the_safe_edges_of_the_eye() {
    let scale = screen().point_scale().pixels_per_point();
    let inset = CalibrationConfig::default().layout.side_button_inset_pt;
    // Слева и справа глазами — какой край поверхности это (как в тесте безопасной зоны).
    for (orientation, (left_px, right_px)) in [
        (DeviceOrientation::Portrait, (10.0, 30.0)),
        (DeviceOrientation::LandscapeLeft, (20.0, 40.0)),
        (DeviceOrientation::PortraitUpsideDown, (30.0, 10.0)),
        (DeviceOrientation::LandscapeRight, (40.0, 20.0)),
    ] {
        let plain = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let safe = calibration_layout_in(orientation, distinct_insets()).expect("раскладка");
        let eye_width = plain.eye_size().width;
        assert!(
            (safe.menu_button_center().x - (eye_width - right_px / scale - inset)).abs() < 1e-6,
            "{orientation:?}: меню отступает от правого края безопасной зоны"
        );
        assert!(
            (safe.clean_button_center().x - (left_px / scale + inset)).abs() < 1e-6,
            "{orientation:?}: CLEAN отступает от левого края безопасной зоны"
        );
        assert_eq!(
            safe.carousel_center().x,
            plain.carousel_center().x,
            "капсула по центру ширины экрана"
        );
    }
}

#[test]
fn both_buttons_stay_inside_the_owner_safe_area_sideways_and_their_centres_stay_inside_everywhere() {
    let scale = screen().point_scale().pixels_per_point();
    let diameter = ScreenConfig::default().widgets.bottom.menu_button.diameter_pt;
    let surface = screen().screen_size();
    let safe_surface = Rect::from_xywh(
        0.0,
        156.0 / scale,
        surface.width,
        surface.height - (156.0 + 132.0) / scale,
    );
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout_in(orientation, owner_insets()).expect("раскладка");
        for (name, centre) in [
            ("меню", layout.menu_button_center()),
            ("CLEAN", layout.clean_button_center()),
        ] {
            let on_surface = layout.eye_point_on_surface(centre);
            let button = Rect::centered_at(on_surface, Size::new(diameter, diameter));
            assert!(
                button.min_x() >= safe_surface.min_x() - 1e-6 && button.max_x() <= safe_surface.max_x() + 1e-6,
                "{orientation:?}: {name} по ширине в безопасной зоне: {button:?}"
            );
            // Вверх ногами владелец раздел не держит (`HELD` в тестах экрана): там линия капсулы PHOTO попадает в
            // верхнюю полосу, и центр кнопки в безопасной зоне не лежит — это цена «линии капсулы».
            if orientation != DeviceOrientation::PortraitUpsideDown {
                assert!(
                    safe_surface.contains(on_surface),
                    "{orientation:?}: центр кнопки {name} в безопасной зоне: {on_surface:?}"
                );
            }
            // В портрете капсула PHOTO стоит низко (линия 898 + сдвиг), и нижний край кнопки Ø44 заходит в полосу
            // системы: это цена решения «на линии капсулы». В альбоме кнопка целиком внутри.
            if matches!(
                orientation,
                DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight
            ) {
                assert!(
                    button.min_y() >= safe_surface.min_y() - 1e-6 && button.max_y() <= safe_surface.max_y() + 1e-6,
                    "{orientation:?}: {name} целиком в безопасной зоне: {button:?}"
                );
            }
        }
    }
}
