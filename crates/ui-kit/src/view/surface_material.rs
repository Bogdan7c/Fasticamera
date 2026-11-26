//! Поверхность панели без стекла (14m): тот же член на том же месте, но обычная закрашенная фигура.
//!
//! Поверхность — это `DrawItem::Shape` с той же геометрией и тем же преобразованием, что у члена
//! группы стекла (включая запечённый масштаб нажатия), заливкой «цвет × плотность × появление» и
//! без тени и свечения. Слой и порядок рисования остаются прежними (поверхность ставится туда, где
//! стояло бы стекло), поэтому содержимое (подписи, значки) по-прежнему идёт поверх.
//!
//! Нажатие: тот же масштаб, что у стеклянной кнопки (`PressParamsPx::scale_for`), без свечения.
//! Появление: плотность растёт с нуля тем же прогрессом, что материализация стекла (решение 270).
//! Ободок (роль `EdgedPanel`, PHOTO, решение 297): светлая обводка по внутреннему краю силуэта, без тени.

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, PressState, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
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
/// * `appear` — появление 0..1 (у стекла это `effect`): плотность заливки и альфа ободка умножаются на него;
/// * `opacity` — прозрачность узла (растворение узла растворяет и поверхность).
///
/// У роли с ободком ([`SurfaceRole::EdgedPanel`], решение 297) фигура заливки уже на толщину линии, а линия
/// идёт по её краю: обводка `ui-draw` центрирована на границе, поэтому внешний край линии ложится ровно на
/// силуэт члена. Силуэт с ободком и без него один и тот же.
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
    let (geometry, stroke) = match tokens.surface.edge_for(role, appear) {
        None => (member_geometry(shape, placement.size, scale), None),
        Some(edge) => {
            let width_px = scale.px(f64::from(edge.width_pt));
            let stroke = StrokeStyle {
                width_px,
                color: edge.color,
            };
            (
                inset_member_geometry(shape, placement.size, scale, width_px),
                Some(stroke),
            )
        }
    };
    let item = ShapeItem {
        geometry,
        transform,
        fill: tokens.surface.fill_for(role, appear),
        stroke,
        shadow: None,
        opacity,
    };
    Ok((item, transform))
}

/// Геометрия члена, ужатая на `edge_px` по каждой оси (на половину линии с каждой стороны): по её краю
/// идёт центрированная обводка толщиной `edge_px`, и внешний край обводки совпадает с краем полной формы.
/// Радиус скругления уменьшается на ту же половину линии, иначе внешний край линии на углах ушёл бы внутрь.
fn inset_member_geometry(shape: GlassShape, size: PxSize, scale: PxPerPt, edge_px: f32) -> ShapeGeometry {
    let inset_size = PxSize::new((size.width - edge_px).max(0.0), (size.height - edge_px).max(0.0));
    match shape {
        GlassShape::Capsule | GlassShape::Circle => member_geometry(shape, inset_size, scale),
        GlassShape::RoundedRect { corner_radius_pt } => {
            let half_edge_pt = 0.5 * edge_px / scale.get();
            member_geometry(
                GlassShape::RoundedRect {
                    corner_radius_pt: (corner_radius_pt - half_edge_pt).max(0.0),
                },
                inset_size,
                scale,
            )
        }
    }
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
    use ui_draw::geometry::PxPoint;

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

    /// Поверхность роли `role` для формы `shape` на тестовом размещении (300 × 90 px в точке (500; 400)).
    fn surface_of(role: SurfaceRole, shape: GlassShape, appear: f32) -> ShapeItem {
        surface_member(
            &UiKitTokens::default(),
            role,
            MemberSite {
                shape,
                placement: &placement(),
                press: PressState::RELEASED,
            },
            scale(),
            appear,
            Opacity::OPAQUE,
        )
        .expect("поверхность")
        .0
    }

    /// Пиксель эталона `ui-draw` (premultiplied) в точке `(x, y)` экрана, сглаживание 1 px, как в шейдере.
    fn pixel(item: &ShapeItem, x: f32, y: f32) -> ui_draw::color::PremultipliedColor {
        ui_draw::draw_list::shade_shape_pixel(item, PxPoint::new(x, y), 1.0).expect("пиксель")
    }

    #[test]
    fn edged_panel_draws_a_light_edge_inside_the_same_silhouette() {
        // Решение 297: светлая линия 1 pt (3 px при масштабе 3) по внутреннему краю, силуэт прежний.
        for shape in [
            GlassShape::Capsule,
            GlassShape::Circle,
            GlassShape::RoundedRect { corner_radius_pt: 20.0 },
        ] {
            let plain = surface_of(SurfaceRole::Panel, shape, 1.0);
            let edged = surface_of(SurfaceRole::EdgedPanel, shape, 1.0);
            // Правый край силуэта на горизонтали центра: x = 500 + половина ширины формы.
            let half_width = match plain.geometry {
                ShapeGeometry::Circle { diameter } => diameter / 2.0,
                ShapeGeometry::Capsule { size } | ShapeGeometry::RoundedRect { size, .. } => size.width / 2.0,
            };
            let edge_x = 500.0 + half_width;
            // Внутри линии (1,5 px от края силуэта) ободок светлее, чем заливка без ободка (premultiplied —
            // это и есть яркость пикселя поверх чёрного фона).
            let band_plain = pixel(&plain, edge_x - 1.5, 400.0);
            let band_edged = pixel(&edged, edge_x - 1.5, 400.0);
            assert!(
                band_edged.r > band_plain.r + 0.1,
                "{shape:?}: линия ободка светлее заливки ({} против {})",
                band_edged.r,
                band_plain.r
            );
            // Центр — та же заливка, что у панели раздела: ободок не меняет плотность.
            assert_eq!(
                pixel(&edged, 500.0, 400.0),
                pixel(&plain, 500.0, 400.0),
                "{shape:?}: центр"
            );
            // Снаружи силуэта (1 px за краем, вне сглаживания) ничего нет — ободок не раздувает элемент.
            assert_eq!(pixel(&edged, edge_x + 1.0, 400.0).a, 0.0, "{shape:?}: снаружи пусто");
            assert_eq!(
                pixel(&plain, edge_x + 1.0, 400.0).a,
                0.0,
                "{shape:?}: снаружи пусто без ободка"
            );
            // Глубже линии (6 px от края) снова обычная заливка.
            assert_eq!(
                pixel(&edged, edge_x - 6.0, 400.0),
                pixel(&plain, edge_x - 6.0, 400.0),
                "{shape:?}: под линией"
            );
            let stroke = edged.stroke.expect("у панели с ободком есть линия");
            assert_eq!(stroke.width_px, 3.0, "{shape:?}: толщина 1 pt");
            assert_eq!(stroke.color.a, 0.22, "{shape:?}: альфа токена");
            assert!(plain.stroke.is_none(), "{shape:?}: у панели раздела линии нет");
        }
    }

    #[test]
    fn edge_appears_together_with_the_fill() {
        // Решение 270: появление растит и заливку, и линию с нуля одним прогрессом.
        let shape = GlassShape::RoundedRect { corner_radius_pt: 20.0 };
        let half = surface_of(SurfaceRole::EdgedPanel, shape, 0.5);
        let full = surface_of(SurfaceRole::EdgedPanel, shape, 1.0);
        let hidden = surface_of(SurfaceRole::EdgedPanel, shape, 0.0);
        let stroke_alpha = |item: &ShapeItem| item.stroke.expect("линия").color.a;
        assert!((stroke_alpha(&half) - 0.5 * stroke_alpha(&full)).abs() < 1e-6);
        assert!((half.fill.a - 0.5 * full.fill.a).abs() < 1e-6);
        assert_eq!(stroke_alpha(&hidden), 0.0);
        assert_eq!(hidden.fill.a, 0.0);
        // Невидимая поверхность не даёт ни одного пикселя, в том числе на линии.
        assert_eq!(pixel(&hidden, 500.0 + 150.0 - 1.5, 400.0).a, 0.0);
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
    fn glass_material_is_bit_identical_to_the_legacy_single_member_group() {
        // Стекло через `single_member_item` равно тому, что строит прямой вызов строителя (ветка стекла кирпича прежняя).
        let tokens = UiKitTokens::default();
        for (role, shape) in [
            (GlassRole::Button, GlassShape::Capsule),
            (GlassRole::Button, GlassShape::Circle),
            (GlassRole::Panel, GlassShape::RoundedRect { corner_radius_pt: 20.0 }),
        ] {
            let press = pressed(0.6);
            let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(role), scale());
            let member = builder.add_member(shape, placement(), press).expect("член");
            let legacy_group = builder.build(1.0, Opacity::OPAQUE).expect("группа");
            let legacy_host = member_transform(&legacy_group, member).expect("хост");
            let (item, host) = single_member_item(
                &tokens,
                PanelMaterial::Glass(role),
                MemberSite {
                    shape,
                    placement: &placement(),
                    press,
                },
                scale(),
                Opacity::OPAQUE,
            )
            .expect("через материал");
            assert_eq!(
                item,
                DrawItem::Glass(legacy_group),
                "{role:?}: список рисования стекла прежний"
            );
            assert_eq!(host, legacy_host, "{role:?}: хост содержимого прежний");
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
