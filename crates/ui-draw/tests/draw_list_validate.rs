#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Список рисования: проверка каждого вида ошибки, границы элементов, поле слитой группы стекла.

use ui_draw::atlas::AtlasRegion;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{
    DrawItem, DrawList, DrawListError, GlassGroupItem, GlassMember, IconItem, MAX_GLASS_GROUP_MEMBERS, PressState,
    ShapeItem, StrokeStyle, TextItem,
};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::{GlassStyle, ShadowParamsPx};
use ui_draw::icons::IconId;
use ui_draw::light::LightDirection;
use ui_draw::shape::{CornerSpec, ShapeError, ShapeGeometry};
use ui_draw::text::{PlacedGlyph, TextRun};
use ui_draw::transform::{Affine2, AffineError};

fn light() -> LightDirection {
    LightDirection::from_angle_degrees(45.0)
}

fn shape_item() -> ShapeItem {
    ShapeItem {
        geometry: ShapeGeometry::RoundedRect {
            size: PxSize::new(200.0, 100.0),
            corner: CornerSpec::continuous(30.0),
        },
        transform: Affine2::translate(300.0, 400.0),
        fill: DisplayColor::rgb(0.2, 0.4, 0.6),
        stroke: None,
        shadow: None,
        opacity: Opacity::OPAQUE,
    }
}

fn glass_member(center_x: f32) -> GlassMember {
    GlassMember {
        geometry: ShapeGeometry::Capsule {
            size: PxSize::new(200.0, 100.0),
        },
        transform: Affine2::translate(center_x, 500.0),
        press: PressState::RELEASED,
    }
}

fn glass_group(members: Vec<GlassMember>) -> GlassGroupItem {
    GlassGroupItem {
        params: GlassStyle::default().resolve(PxSize::new(200.0, 100.0), 2.4545),
        members,
        merge_radius_px: 40.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

fn text_item() -> TextItem {
    let glyph = PlacedGlyph {
        region: AtlasRegion {
            uv_min: [0.0, 0.0],
            uv_max: [0.1, 0.1],
        },
        quad: PxRect::new(PxPoint::new(0.0, -20.0), PxSize::new(15.0, 20.0)),
    };
    let second = PlacedGlyph {
        quad: PxRect::new(PxPoint::new(16.0, -22.0), PxSize::new(14.0, 24.0)),
        ..glyph.clone()
    };
    TextItem {
        run: TextRun {
            glyphs: vec![glyph, second],
            advance_px: 30.0,
            cap_height_px: 20.0,
        },
        transform: Affine2::translate(100.0, 100.0),
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }
}

fn icon_item() -> IconItem {
    IconItem {
        icon: IconId::Grid,
        transform: Affine2::translate(50.0, 60.0),
        size_px: PxSize::new(48.0, 48.0),
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }
}

fn list_of(items: Vec<DrawItem>) -> DrawList {
    DrawList { items, light: light() }
}

fn valid_list() -> DrawList {
    list_of(vec![
        DrawItem::Shape(shape_item()),
        DrawItem::Glass(glass_group(vec![glass_member(200.0), glass_member(450.0)])),
        DrawItem::Text(text_item()),
        DrawItem::Icon(icon_item()),
    ])
}

#[test]
fn valid_list_passes_and_empty_list_passes() {
    assert_eq!(valid_list().validate(), Ok(()));
    assert_eq!(DrawList::new(light()).validate(), Ok(()));
}

#[test]
fn nan_in_a_color_or_transform_is_reported_with_the_item_index() {
    let mut shape = shape_item();
    shape.fill = DisplayColor::new(f32::NAN, 0.0, 0.0, 1.0);
    let list = list_of(vec![DrawItem::Icon(icon_item()), DrawItem::Shape(shape)]);
    assert_eq!(
        list.validate(),
        Err(DrawListError::NotFinite { item: 1, field: "fill" })
    );

    let mut shape = shape_item();
    shape.transform = Affine2::translate(f32::NAN, 0.0);
    assert_eq!(
        list_of(vec![DrawItem::Shape(shape)]).validate(),
        Err(DrawListError::DegenerateTransform {
            item: 0,
            source: AffineError::NotFinite
        })
    );
}

#[test]
fn negative_sizes_are_rejected() {
    let mut shape = shape_item();
    shape.stroke = Some(StrokeStyle {
        width_px: -2.0,
        color: DisplayColor::WHITE,
    });
    assert_eq!(
        list_of(vec![DrawItem::Shape(shape)]).validate(),
        Err(DrawListError::InvalidSize {
            item: 0,
            field: "stroke.width_px",
            value: -2.0
        })
    );

    let mut shape = shape_item();
    shape.geometry = ShapeGeometry::Capsule {
        size: PxSize::new(-10.0, 10.0),
    };
    assert_eq!(
        list_of(vec![DrawItem::Shape(shape)]).validate(),
        Err(DrawListError::InvalidShape {
            item: 0,
            source: ShapeError::InvalidDimension {
                field: "width",
                value: -10.0
            }
        })
    );

    let mut icon = icon_item();
    icon.size_px = PxSize::new(0.0, 24.0);
    assert!(matches!(
        list_of(vec![DrawItem::Icon(icon)]).validate(),
        Err(DrawListError::InvalidSize {
            item: 0,
            field: "size_px",
            ..
        })
    ));

    let mut text = text_item();
    text.run.advance_px = -1.0;
    assert!(matches!(
        list_of(vec![DrawItem::Text(text)]).validate(),
        Err(DrawListError::InvalidSize {
            item: 0,
            field: "run.advance_px",
            ..
        })
    ));
}

#[test]
fn degenerate_transform_is_rejected() {
    let mut shape = shape_item();
    shape.transform = Affine2::scale(0.0, 1.0);
    assert!(matches!(
        list_of(vec![DrawItem::Shape(shape)]).validate(),
        Err(DrawListError::DegenerateTransform {
            item: 0,
            source: AffineError::Degenerate { .. }
        })
    ));
}

#[test]
fn glass_group_limits_and_ranges_are_enforced() {
    let at_limit = (0..MAX_GLASS_GROUP_MEMBERS)
        .map(|index| glass_member(index as f32 * 10.0))
        .collect();
    assert_eq!(list_of(vec![DrawItem::Glass(glass_group(at_limit))]).validate(), Ok(()));

    let over_limit = (0..=MAX_GLASS_GROUP_MEMBERS)
        .map(|index| glass_member(index as f32 * 10.0))
        .collect();
    assert_eq!(
        list_of(vec![DrawItem::Glass(glass_group(over_limit))]).validate(),
        Err(DrawListError::TooManyGlassMembers {
            item: 0,
            count: MAX_GLASS_GROUP_MEMBERS + 1,
            limit: MAX_GLASS_GROUP_MEMBERS
        })
    );
    assert_eq!(
        list_of(vec![DrawItem::Glass(glass_group(Vec::new()))]).validate(),
        Err(DrawListError::EmptyGlassGroup { item: 0 })
    );

    let mut group = glass_group(vec![glass_member(100.0)]);
    group.effect = 1.5;
    assert_eq!(
        list_of(vec![DrawItem::Glass(group)]).validate(),
        Err(DrawListError::OutOfUnitRange {
            item: 0,
            field: "effect",
            value: 1.5
        })
    );

    let mut group = glass_group(vec![glass_member(100.0)]);
    group.members[0].press.amount = -0.1;
    assert!(matches!(
        list_of(vec![DrawItem::Glass(group)]).validate(),
        Err(DrawListError::OutOfUnitRange {
            field: "press.amount",
            ..
        })
    ));

    // resolve с px_per_pt = NaN даёт нечисловые параметры — их ловит проверка группы.
    let mut group = glass_group(vec![glass_member(100.0)]);
    group.params = GlassStyle::default().resolve(PxSize::new(200.0, 100.0), f32::NAN);
    assert!(matches!(
        list_of(vec![DrawItem::Glass(group)]).validate(),
        Err(DrawListError::InvalidSize {
            field: "params.blur_radius_px",
            ..
        })
    ));

    let mut group = glass_group(vec![glass_member(100.0)]);
    group.members[0].transform = Affine2::scale(0.0, 0.0);
    assert!(matches!(
        list_of(vec![DrawItem::Glass(group)]).validate(),
        Err(DrawListError::DegenerateTransform { .. })
    ));
}

#[test]
fn opacity_cannot_be_out_of_range() {
    assert!(Opacity::new(1.2).is_err());
    assert!(Opacity::new(f32::NAN).is_err());
    assert!(serde_json::from_str::<Opacity>("2.0").is_err(), "и через serde тоже");
    assert_eq!(Opacity::new(0.25).unwrap().get(), 0.25);
}

#[test]
fn shape_bounds_cover_the_transformed_box_stroke_and_shadow() {
    let plain = DrawItem::Shape(shape_item()).bounds_px().unwrap();
    assert_eq!(
        plain,
        PxRect::new(PxPoint::new(200.0, 350.0), PxSize::new(200.0, 100.0))
    );

    let mut stroked = shape_item();
    stroked.stroke = Some(StrokeStyle {
        width_px: 10.0,
        color: DisplayColor::WHITE,
    });
    let with_stroke = DrawItem::Shape(stroked).bounds_px().unwrap();
    assert_eq!(
        with_stroke,
        PxRect::new(PxPoint::new(195.0, 345.0), PxSize::new(210.0, 110.0))
    );

    let mut shadowed = shape_item();
    shadowed.shadow = Some(ShadowParamsPx {
        radius_px: 20.0,
        alpha: 0.1,
        offset_px: 5.0,
    });
    let with_shadow = DrawItem::Shape(shadowed).bounds_px().unwrap();
    assert!(with_shadow.size.width >= plain.size.width + 40.0 && with_shadow.contains(PxPoint::new(300.0, 475.0)));

    // Поворот на 90° меняет ширину и высоту местами.
    let mut rotated = shape_item();
    rotated.transform = Affine2::rotate(std::f32::consts::FRAC_PI_2).then(Affine2::translate(300.0, 400.0));
    let rotated_bounds = DrawItem::Shape(rotated).bounds_px().unwrap();
    assert!((rotated_bounds.size.width - 100.0).abs() < 1e-3 && (rotated_bounds.size.height - 200.0).abs() < 1e-3);
}

#[test]
fn text_and_icon_bounds_follow_their_geometry() {
    let text = DrawItem::Text(text_item()).bounds_px().unwrap();
    // Глифы: x 0…30, y −22…2 → +сдвиг (100,100).
    assert_eq!(text, PxRect::new(PxPoint::new(100.0, 78.0), PxSize::new(30.0, 24.0)));
    let mut empty = text_item();
    empty.run.glyphs.clear();
    assert_eq!(
        DrawItem::Text(empty).bounds_px(),
        None,
        "пустая строка — нечего рисовать"
    );
    let icon = DrawItem::Icon(icon_item()).bounds_px().unwrap();
    assert_eq!(icon, PxRect::new(PxPoint::new(26.0, 36.0), PxSize::new(48.0, 48.0)));
}

#[test]
fn glass_bounds_include_every_member_and_the_margin_for_shadow_and_lens() {
    let group = glass_group(vec![glass_member(200.0), glass_member(450.0)]);
    let bounds = DrawItem::Glass(group.clone()).bounds_px().unwrap();
    // Члены: x от 100 до 550, y от 450 до 550.
    let members_only = PxRect::new(PxPoint::new(100.0, 450.0), PxSize::new(450.0, 100.0));
    assert!(bounds.size.width > members_only.size.width && bounds.size.height > members_only.size.height);
    for corner in [PxPoint::new(100.0, 450.0), PxPoint::new(550.0, 550.0)] {
        assert!(bounds.contains(corner));
    }
    let margin = group.params.shadow.radius_px + group.params.shadow.offset_px;
    assert!(
        bounds.origin.x <= 100.0 - margin + 1e-3,
        "запас под тень, {}",
        bounds.origin.x
    );
    assert!(
        bounds.origin.x <= 100.0 - group.params.refraction_strength_px,
        "запас под вынос линзы"
    );
    assert_eq!(DrawItem::Glass(glass_group(Vec::new())).bounds_px(), None);
}

#[test]
fn merged_group_field_fills_the_gap_between_close_members_only() {
    // Две капсулы 200×100: центры 200 и 450 → зазор между краями 50 px. Радиус слияния 40 < 2·50.
    let mut group = glass_group(vec![glass_member(200.0), glass_member(450.0)]);
    let gap_middle = PxPoint::new(325.0, 500.0);
    group.merge_radius_px = 40.0;
    let separate = group.field_at(gap_middle).unwrap().unwrap();
    assert!(
        separate.distance_px > 0.0,
        "радиус слияния меньше зазора: между капсулами пустота"
    );
    // Радиус 200: формы слились, между ними стекло и нормаль почти вертикальна (по бокам впадины).
    group.merge_radius_px = 200.0;
    let merged = group.field_at(gap_middle).unwrap().unwrap();
    assert!(
        merged.distance_px < separate.distance_px - 10.0,
        "слияние обязано углубить поле: {merged:?}"
    );
    // Далёкие члены не сливаются, поле на середине как у разделённых.
    let far = glass_group(vec![glass_member(0.0), glass_member(1000.0)]);
    let far_field = far.field_at(PxPoint::new(500.0, 500.0)).unwrap().unwrap();
    assert!(far_field.distance_px > 100.0);
    // Нормаль справа от одной капсулы смотрит наружу (+x), с единичной длиной.
    let single = glass_group(vec![glass_member(200.0)]);
    let right_edge = single.field_at(PxPoint::new(300.5, 500.0)).unwrap().unwrap();
    assert!((right_edge.distance_px - 0.5).abs() < 1e-3);
    assert!(right_edge.normal_out.dx > 0.99);
    assert_eq!(glass_group(Vec::new()).field_at(PxPoint::ORIGIN).unwrap(), None);
}

#[test]
fn group_field_respects_member_scale_and_rejects_degenerate_members() {
    // Капсула 100 px, увеличенная в 2 раза: расстояние до границы считается в экранных px.
    let mut member = glass_member(0.0);
    member.transform = Affine2::uniform_scale(2.0).then(Affine2::translate(0.0, 500.0));
    let group = glass_group(vec![member]);
    // Граница справа в x = 200 (половина ширины 100 × 2). Точка в 10 px снаружи.
    let field = group.field_at(PxPoint::new(210.0, 500.0)).unwrap().unwrap();
    assert!((field.distance_px - 10.0).abs() < 1e-2, "{}", field.distance_px);
    let mut broken = glass_group(vec![glass_member(0.0)]);
    broken.members[0].transform = Affine2::scale(0.0, 1.0);
    assert!(broken.field_at(PxPoint::ORIGIN).is_err());
}

#[test]
fn member_touch_point_is_mapped_to_screen_pixels() {
    let mut member = glass_member(200.0);
    member.press = PressState {
        amount: 1.0,
        touch_point_local: PxPoint::new(10.0, -5.0),
    };
    assert_eq!(member.touch_point_px(), PxPoint::new(210.0, 495.0));
}
