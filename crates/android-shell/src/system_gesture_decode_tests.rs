use shell_contract::surface::SurfaceSize;
use shell_contract::system_gesture::ScreenEdge;
use shell_contract::touch::SurfacePoint;

use super::*;

#[test]
fn android_insets_order_left_top_right_bottom_is_mapped_to_the_right_edges() {
    // Числа разные, чтобы любая перестановка аргументов была видна.
    let zones = decode_system_gesture_zones(10, 156, 30, 132).expect("корректные толщины");
    assert_eq!(zones.thickness_px(ScreenEdge::Left), 10);
    assert_eq!(zones.thickness_px(ScreenEdge::Top), 156);
    assert_eq!(zones.thickness_px(ScreenEdge::Right), 30);
    assert_eq!(zones.thickness_px(ScreenEdge::Bottom), 132);
}

#[test]
fn phone_report_decodes_into_zones_that_claim_the_bottom_swipe_start() {
    // Сообщение телефона владельца: сверху 156 px, снизу 132 px.
    let zones = decode_system_gesture_zones(0, 156, 0, 132).expect("корректные толщины");
    let surface = SurfaceSize {
        width_px: 1080,
        height_px: 2412,
    };
    assert!(zones.contains(
        surface,
        SurfacePoint {
            x_px: 540.0,
            y_px: 2400.0
        }
    ));
    assert!(!zones.contains(
        surface,
        SurfacePoint {
            x_px: 540.0,
            y_px: 2200.0
        }
    ));
}

#[test]
fn all_zero_insets_mean_no_zones() {
    assert!(decode_system_gesture_zones(0, 0, 0, 0).expect("нули").is_none());
}

#[test]
fn negative_thickness_is_an_error_not_clamped() {
    let error = decode_system_gesture_zones(0, 0, 0, -1).expect_err("отрицательная толщина");
    assert_eq!(
        error,
        SystemGestureZonesError::NegativeThickness {
            edge: ScreenEdge::Bottom,
            thickness_px: -1
        }
    );
}
