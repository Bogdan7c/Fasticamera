use super::*;

#[test]
fn each_edge_keeps_its_own_number() {
    let insets = EdgeInsetsPx::try_from_raw(RawEdgeInsetsPx {
        left: 1,
        top: 2,
        right: 3,
        bottom: 4,
    })
    .expect("корректные толщины");
    assert_eq!(insets.thickness_px(ScreenEdge::Left), 1);
    assert_eq!(insets.thickness_px(ScreenEdge::Top), 2);
    assert_eq!(insets.thickness_px(ScreenEdge::Right), 3);
    assert_eq!(insets.thickness_px(ScreenEdge::Bottom), 4);
}

#[test]
fn a_negative_number_names_the_offending_edge_for_every_edge() {
    let zero = RawEdgeInsetsPx {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    for (edge, raw) in [
        (ScreenEdge::Left, RawEdgeInsetsPx { left: -7, ..zero }),
        (ScreenEdge::Top, RawEdgeInsetsPx { top: -7, ..zero }),
        (ScreenEdge::Right, RawEdgeInsetsPx { right: -7, ..zero }),
        (ScreenEdge::Bottom, RawEdgeInsetsPx { bottom: -7, ..zero }),
    ] {
        assert_eq!(
            EdgeInsetsPx::try_from_raw(raw),
            Err(EdgeInsetsError::NegativeThickness { edge, thickness_px: -7 })
        );
    }
    assert_eq!(EdgeInsetsPx::try_from_raw(zero), Ok(EdgeInsetsPx::ZERO));
}
