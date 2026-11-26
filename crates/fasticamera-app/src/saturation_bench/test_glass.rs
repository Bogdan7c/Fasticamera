//! Тестовый помощник стенда: группы стекла в известных местах. После 14p ни один экран камеры стекла в покое не
//! рисует (решение 297; стекло — только у завесы перехода), а учёт стекла в составе сцены и фильтры по стеклу
//! по-прежнему должны работать — их проверяют на этих группах.

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, PressState};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::tokens::{GlassRole, UiKitTokens};
use ui_kit::view::{GlassGroupBuilder, GlassShape, PxPerPt};

/// Две панели стекла (роль Panel) на экране 1080 px шириной: в середине экрана и свисающая за левый край.
pub(crate) fn two_glass_panels() -> Vec<DrawItem> {
    let tokens = UiKitTokens::default();
    let scale = PxPerPt::new(1080.0 / 440.0).expect("масштаб");
    [((540.0, 1200.0), (400.0, 300.0)), ((40.0, 600.0), (300.0, 200.0))]
        .into_iter()
        .map(|(center, size)| {
            let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(GlassRole::Panel), scale);
            builder
                .add_member(
                    GlassShape::RoundedRect { corner_radius_pt: 10.0 },
                    NodePlacementPx {
                        size: PxSize::new(size.0, size.1),
                        transform: Affine2::translate(center.0, center.1),
                        opacity: 1.0,
                    },
                    PressState::RELEASED,
                )
                .expect("член");
            DrawItem::Glass(builder.build(1.0, Opacity::OPAQUE).expect("группа"))
        })
        .collect()
}
