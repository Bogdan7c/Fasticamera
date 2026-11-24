use shell_contract::edge_insets::ScreenEdge;

use super::*;

#[test]
fn android_insets_order_left_top_right_bottom_is_mapped_to_the_right_edges() {
    // Числа разные, чтобы любая перестановка аргументов была видна.
    let area = decode_safe_area(10, 156, 30, 132).expect("корректные отступы");
    assert_eq!(area.thickness_px(ScreenEdge::Left), 10);
    assert_eq!(area.thickness_px(ScreenEdge::Top), 156);
    assert_eq!(area.thickness_px(ScreenEdge::Right), 30);
    assert_eq!(area.thickness_px(ScreenEdge::Bottom), 132);
}

#[test]
fn all_zero_insets_mean_the_whole_screen_is_safe() {
    assert_eq!(decode_safe_area(0, 0, 0, 0), Ok(SafeAreaInsets::NONE));
}

#[test]
fn a_negative_inset_is_an_error_not_clamped() {
    assert_eq!(
        decode_safe_area(-3, 0, 0, 0),
        Err(EdgeInsetsError::NegativeThickness {
            edge: ScreenEdge::Left,
            thickness_px: -3
        })
    );
}
