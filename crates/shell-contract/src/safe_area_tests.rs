use super::*;

#[test]
fn the_phone_report_keeps_every_edge_and_the_default_is_no_insets() {
    let area = SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 5,
        top: 156,
        right: 7,
        bottom: 132,
    })
    .expect("отступы телефона");
    assert_eq!(area.thickness_px(ScreenEdge::Left), 5);
    assert_eq!(area.thickness_px(ScreenEdge::Top), 156);
    assert_eq!(area.thickness_px(ScreenEdge::Right), 7);
    assert_eq!(area.thickness_px(ScreenEdge::Bottom), 132);

    assert_eq!(SafeAreaInsets::default(), SafeAreaInsets::NONE);
    for edge in [ScreenEdge::Left, ScreenEdge::Top, ScreenEdge::Right, ScreenEdge::Bottom] {
        assert_eq!(SafeAreaInsets::NONE.thickness_px(edge), 0);
    }
}

#[test]
fn a_negative_inset_is_an_error_not_zero() {
    let error = SafeAreaInsets::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: -1,
        right: 0,
        bottom: 0,
    })
    .expect_err("отрицательный отступ");
    assert_eq!(
        error,
        EdgeInsetsError::NegativeThickness {
            edge: ScreenEdge::Top,
            thickness_px: -1
        }
    );
}
