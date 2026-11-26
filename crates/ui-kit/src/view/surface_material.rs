//! Поверхность панели без стекла (14m): тот же член на том же месте, но обычная закрашенная фигура.
//!
//! Поверхность — это `DrawItem::Shape` с той же геометрией и тем же преобразованием, что у члена
//! группы стекла (включая запечённый масштаб нажатия), заливкой «цвет × плотность × появление» и
//! без тени и свечения. Слой и порядок рисования остаются прежними (поверхность ставится туда, где
//! стояло бы стекло), поэтому содержимое (подписи, значки) по-прежнему идёт поверх.
//!
//! Нажатие: тот же масштаб, что у стеклянной кнопки (`PressParamsPx::scale_for`), без свечения.
//! Появление: плотность растёт с нуля тем же прогрессом, что материализация стекла (решение 270).

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, PressState, ShapeItem};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::glass_surface::{GlassGroupBuilder, GlassShape, member_geometry, member_transform};
use super::scale::PxPerPt;
use crate::tokens::{PanelMaterial, SurfaceRole, UiKitTokens};

/// Где и как стоит один член: форма, размещение узла сцены и нажатие. Общий вход для стекла и поверхности.
#[derive(Debug, Clone, Copy)]
pub struct MemberSite<'a> {
    pub shape: GlassShape,
    pub placement: &'a NodePlacementPx,
    pub press: PressState,
}

/// Поверхность одного члена: фигура и преобразование, в котором ставится содержимое члена (с масштабом
/// нажатия — так же, как у [`member_transform`] группы стекла).
///
/// * `appear` — появление 0..1 (у стекла это `effect`): плотность заливки умножается на него;
/// * `opacity` — прозрачность узла (растворение узла растворяет и поверхность).
pub fn surface_member(
    tokens: &UiKitTokens,
    role: SurfaceRole,
    site: MemberSite<'_>,
    scale: PxPerPt,
    appear: f32,
    opacity: Opacity,
) -> Result<(ShapeItem, Affine2), ViewError> {
    if !appear.is_finite() || !(0.0..=1.0).contains(&appear) {
        return Err(ViewError::InvalidEffect(appear));
    }
    // Масштаб нажатия берём у пресета кнопки: нажимается капсула режимов и кнопка меню, у стекла это
    // пресет Button. Свечения и преломления у поверхности нет — только масштаб формы.
    let MemberSite {
        shape,
        placement,
        press,
    } = site;
    let press_scale = tokens.glass.button.press.scale_gain;
    let transform = Affine2::uniform_scale(1.0 + press_scale * press.amount.clamp(0.0, 1.0)).then(placement.transform);
    let item = ShapeItem {
        geometry: member_geometry(shape, placement.size, scale),
        transform,
        fill: tokens.surface.fill_for(role, appear),
        stroke: None,
        shadow: None,
        opacity,
    };
    Ok((item, transform))
}

/// Элемент рисования одного члена выбранного материала и преобразование его содержимого.
///
/// Стекло собирается ровно как раньше (`GlassGroupBuilder` с одним членом, материализация 1), поэтому
/// PHOTO остаётся бит в бит прежним; поверхность — [`surface_member`] с появлением 1.
pub fn single_member_item(
    tokens: &UiKitTokens,
    material: PanelMaterial,
    site: MemberSite<'_>,
    scale: PxPerPt,
    opacity: Opacity,
) -> Result<(DrawItem, Affine2), ViewError> {
    let MemberSite {
        shape,
        placement,
        press,
    } = site;
    match material {
        PanelMaterial::Glass(role) => {
            let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(role), scale);
            let member = builder.add_member(shape, *placement, press)?;
            let group = builder.build(1.0, opacity)?;
            let host = member_transform(&group, member)?;
            Ok((DrawItem::Glass(group), host))
        }
        PanelMaterial::Surface(role) => {
            // Появление 1: плотность роли целиком (у кнопок режима нет анимации появления).
            let (item, host) = surface_member(tokens, role, site, scale, 1.0, opacity)?;
            Ok((DrawItem::Shape(item), host))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui_draw::draw_list::DrawItem;
    use ui_draw::geometry::{PxPoint, PxSize};
    use ui_draw::shape::ShapeGeometry;

    use crate::tokens::GlassRole;

    fn scale() -> PxPerPt {
        PxPerPt::new(3.0).expect("масштаб")
    }

    fn placement() -> NodePlacementPx {
        NodePlacementPx {
            size: PxSize::new(300.0, 90.0),
            transform: Affine2::translate(500.0, 400.0),
            opacity: 1.0,
        }
    }

    fn pressed(amount: f32) -> PressState {
        PressState {
            amount,
            touch_point_local: PxPoint::ORIGIN,
        }
    }

    #[test]
    fn surface_has_glass_geometry_and_density_fill_without_shadow() {
        let tokens = UiKitTokens::default();
        let (surface, _) = surface_member(
            &tokens,
            SurfaceRole::Panel,
            MemberSite {
                shape: GlassShape::Capsule,
                placement: &placement(),
                press: PressState::RELEASED,
            },
            scale(),
            1.0,
            Opacity::OPAQUE,
        )
        .expect("поверхность");
        let (glass, _) = single_member_item(
            &tokens,
            PanelMaterial::Glass(GlassRole::Button),
            MemberSite {
                shape: GlassShape::Capsule,
                placement: &placement(),
                press: PressState::RELEASED,
            },
            scale(),
            Opacity::OPAQUE,
        )
        .expect("стекло");
        let DrawItem::Glass(group) = glass else {
            panic!("стекло должно остаться стеклом");
        };
        // Та же форма и то же место, что у члена стекла.
        assert_eq!(surface.geometry, group.members[0].geometry);
        assert_eq!(surface.transform, group.members[0].transform);
        // Заливка = цвет и плотность из токенов; ни тени, ни обводки.
        assert_eq!(surface.fill, tokens.surface.fill_color.with_alpha(0.70));
        assert!(surface.shadow.is_none() && surface.stroke.is_none());
    }

    #[test]
    fn backing_is_denser_than_panel_and_appear_scales_density() {
        let tokens = UiKitTokens::default();
        let build = |role, appear| {
            surface_member(
                &tokens,
                role,
                MemberSite {
                    shape: GlassShape::Capsule,
                    placement: &placement(),
                    press: PressState::RELEASED,
                },
                scale(),
                appear,
                Opacity::OPAQUE,
            )
            .expect("поверхность")
            .0
            .fill
            .a
        };
        assert!(build(SurfaceRole::Backing, 1.0) > build(SurfaceRole::Panel, 1.0));
        assert!((build(SurfaceRole::Panel, 0.5) - 0.35).abs() < 1e-6);
        assert_eq!(build(SurfaceRole::Panel, 0.0), 0.0);
    }

    #[test]
    fn press_scales_like_glass_button_and_adds_no_glow() {
        let tokens = UiKitTokens::default();
        let (released, released_host) = surface_member(
            &tokens,
            SurfaceRole::Panel,
            MemberSite {
                shape: GlassShape::Capsule,
                placement: &placement(),
                press: PressState::RELEASED,
            },
            scale(),
            1.0,
            Opacity::OPAQUE,
        )
        .expect("отпущена");
        let (down, down_host) = surface_member(
            &tokens,
            SurfaceRole::Panel,
            MemberSite {
                shape: GlassShape::Capsule,
                placement: &placement(),
                press: pressed(1.0),
            },
            scale(),
            1.0,
            Opacity::OPAQUE,
        )
        .expect("нажата");
        let (_, glass_host) = single_member_item(
            &tokens,
            PanelMaterial::Glass(GlassRole::Button),
            MemberSite {
                shape: GlassShape::Capsule,
                placement: &placement(),
                press: pressed(1.0),
            },
            scale(),
            Opacity::OPAQUE,
        )
        .expect("стекло нажато");
        // Нажатая поверхность масштабируется ровно как нажатая стеклянная кнопка.
        assert_eq!(down_host, glass_host);
        assert_ne!(down_host, released_host);
        assert_eq!(down.transform, down_host);
        // Заливка при нажатии не меняется (свечения нет), тени нет.
        assert_eq!(down.fill, released.fill);
        assert!(down.shadow.is_none());
        assert_eq!(
            released.geometry,
            ShapeGeometry::Capsule {
                size: PxSize::new(300.0, 90.0)
            }
        );
    }

    #[test]
    fn invalid_appear_is_an_error_not_a_clamp() {
        let tokens = UiKitTokens::default();
        for bad in [-0.1, 1.5, f32::NAN] {
            let result = surface_member(
                &tokens,
                SurfaceRole::Panel,
                MemberSite {
                    shape: GlassShape::Capsule,
                    placement: &placement(),
                    press: PressState::RELEASED,
                },
                scale(),
                bad,
                Opacity::OPAQUE,
            );
            assert!(matches!(result, Err(ViewError::InvalidEffect(_))), "появление {bad}");
        }
    }

    #[test]
    fn single_member_item_dispatches_material() {
        let tokens = UiKitTokens::default();
        let item_for = |material| {
            single_member_item(
                &tokens,
                material,
                MemberSite {
                    shape: GlassShape::Circle,
                    placement: &placement(),
                    press: PressState::RELEASED,
                },
                scale(),
                Opacity::OPAQUE,
            )
            .expect("элемент")
            .0
        };
        assert!(matches!(
            item_for(PanelMaterial::Glass(GlassRole::Panel)),
            DrawItem::Glass(_)
        ));
        assert!(matches!(
            item_for(PanelMaterial::Surface(SurfaceRole::Backing)),
            DrawItem::Shape(_)
        ));
    }
}
