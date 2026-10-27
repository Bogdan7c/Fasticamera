use camera_contract::raw_format::CfaLayout;

use super::{BayerPattern, CfaCell, RgbChannel, UnsupportedCfa};

#[test]
fn bggr_places_blue_top_left_and_red_bottom_right() {
    let pattern = BayerPattern::from_layout(CfaLayout::Bggr).unwrap();
    assert_eq!(pattern.channel_at(0, 0), RgbChannel::Blue);
    assert_eq!(pattern.channel_at(1, 0), RgbChannel::Green);
    assert_eq!(pattern.channel_at(0, 1), RgbChannel::Green);
    assert_eq!(pattern.channel_at(1, 1), RgbChannel::Red);
    // Период 2: дальние пиксели повторяют квадрат.
    assert_eq!(pattern.channel_at(4097, 3071), RgbChannel::Red);
    assert_eq!(pattern.channel_at(4096, 3070), RgbChannel::Blue);
}

#[test]
fn every_bayer_layout_has_one_red_one_blue_two_green() {
    for layout in [CfaLayout::Rggb, CfaLayout::Grbg, CfaLayout::Gbrg, CfaLayout::Bggr] {
        let pattern = BayerPattern::from_layout(layout).unwrap();
        assert_eq!(pattern.cells_of(RgbChannel::Red).count(), 1, "{layout:?}");
        assert_eq!(pattern.cells_of(RgbChannel::Green).count(), 2, "{layout:?}");
        assert_eq!(pattern.cells_of(RgbChannel::Blue).count(), 1, "{layout:?}");
    }
}

#[test]
fn row_and_column_with_red_match_channel_layout() {
    let pattern = BayerPattern::BGGR;
    assert!(!pattern.row_has_red(0), "строка 0 у BGGR — синяя");
    assert!(pattern.row_has_red(1));
    assert!(!pattern.column_has_red(0));
    assert!(pattern.column_has_red(1));
    let rggb = BayerPattern::RGGB;
    assert!(rggb.row_has_red(0) && rggb.column_has_red(0));
}

#[test]
fn cell_index_follows_camera2_black_level_order() {
    assert_eq!(CfaCell::of_pixel(0, 0).index(), 0);
    assert_eq!(CfaCell::of_pixel(1, 0).index(), 1);
    assert_eq!(CfaCell::of_pixel(0, 1).index(), 2);
    assert_eq!(CfaCell::of_pixel(3, 5).index(), 3);
}

#[test]
fn non_bayer_layouts_are_rejected_not_guessed() {
    for layout in [CfaLayout::Rgb, CfaLayout::Mono, CfaLayout::Nir] {
        assert_eq!(BayerPattern::from_layout(layout), Err(UnsupportedCfa { layout }));
    }
}
