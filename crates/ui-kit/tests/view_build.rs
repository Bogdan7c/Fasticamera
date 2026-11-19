#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Вид K1, K2, K7–K10: геометрия построенных `DrawItem`, поворот узла, проверка `DrawList::validate`,
//! ошибки вместо молчаливой обрезки.

use ui_core::geometry::{Insets, Point, Rect, Size};
use ui_core::time::MotionPreference;
use ui_core::tree::{NodeTransform, NodeVisual, UiTree};
use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, DrawList, MAX_GLASS_GROUP_MEMBERS, PressState};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::icons::IconId;
use ui_draw::light::LightDirection;
use ui_draw::scene_bridge::{NodePlacementPx, node_placement_px};
use ui_draw::shape::ShapeGeometry;
use ui_draw::text::TextError;
use ui_draw::transform::Affine2;
use ui_kit::banner::{Banner, BannerLifetime};
use ui_kit::focus_frame::{FocusFrame, FocusFrameSpec};
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec};
use ui_kit::picker::LabelId;
use ui_kit::shutter::{ShutterButton, ShutterLook};
use ui_kit::tokens::{GlassRole, TextRoleKind, UiKitTokens};
use ui_kit::view::{
    ButtonContent, GlassGroupBuilder, GlassShape, LabelPainter, PxPerPt, ViewError, banner_items, button_content_item,
    centered_icon, focus_frame_items, measure_banner_pt, member_transform, panel_glass_group, shutter_items,
};

mod common;
use common::*;

/// Масштаб телефона: 1320 px / 440 pt = 3 px на pt (решение 82).
const SCALE: f32 = 3.0;

fn scale() -> PxPerPt {
    PxPerPt::new(SCALE).unwrap()
}

fn tokens() -> UiKitTokens {
    UiKitTokens::default()
}

/// Узел размером `width × height` pt с центром в `center`, повёрнутый на `quarter_turns` четвертей.
fn visual(center: Point, width: f64, height: f64, quarter_turns: u32) -> NodeVisual {
    let tree = UiTree::new(Size::new(440.0, 956.0), Insets::default()).unwrap();
    let frame = Rect::centered_at(center, Size::new(width, height));
    // Поворот вокруг центра рамки: образ центра остаётся на месте.
    let (cos, sin) = match quarter_turns % 4 {
        0 => (1.0, 0.0),
        1 => (0.0, 1.0),
        2 => (-1.0, 0.0),
        _ => (0.0, -1.0),
    };
    NodeVisual {
        node: tree.root(),
        label: "node",
        frame,
        to_screen: NodeTransform {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: center.x - (cos * center.x - sin * center.y),
            ty: center.y - (sin * center.x + cos * center.y),
        },
        opacity: 1.0,
        depth: 1,
    }
}

fn placement(center: Point, width: f64, height: f64) -> NodePlacementPx {
    node_placement_px(&visual(center, width, height, 0), f64::from(SCALE))
}

fn centre_px(transform: &Affine2) -> PxPoint {
    transform.apply_point(PxPoint::ORIGIN)
}

fn assert_close(actual: f32, expected: f32, what: &str) {
    assert!((actual - expected).abs() < 0.01, "{what}: {actual} != {expected}");
}

// ---------- K1: стеклянная поверхность ----------

#[test]
fn glass_member_sits_where_the_node_sits_with_the_node_size() {
    let tokens = tokens();
    let node = placement(Point::new(220.0, 897.8), 220.0, 47.8);
    let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(GlassRole::Button), scale());
    let index = builder
        .add_member(GlassShape::Capsule, node, PressState::RELEASED)
        .unwrap();
    let group = builder.build(1.0, Opacity::OPAQUE).unwrap();
    let member = &group.members[index.get()];
    assert_eq!(
        member.geometry,
        ShapeGeometry::Capsule {
            size: PxSize::new(660.0, 143.4)
        }
    );
    let centre = centre_px(&member.transform);
    assert_close(centre.x, 660.0, "центр x");
    assert_close(centre.y, 897.8 * 3.0, "центр y");
}

#[test]
fn pressing_scales_the_member_by_the_glass_press_gain() {
    let tokens = tokens();
    let style = tokens.glass.style_for(GlassRole::Button);
    let node = placement(Point::new(100.0, 100.0), 48.0, 48.0);
    let mut builder = GlassGroupBuilder::new(style, scale());
    builder
        .add_member(
            GlassShape::Circle,
            node,
            PressState {
                amount: 1.0,
                touch_point_local: PxPoint::new(5.0, 0.0),
            },
        )
        .unwrap();
    let group = builder.build(1.0, Opacity::OPAQUE).unwrap();
    let transform = group.members[0].transform;
    // Единичный вектор растягивается на 1 + scale_gain (запечено в член, рендер не добавляет).
    let expected = 1.0 + style.press.scale_gain;
    assert_close(transform.a.hypot(transform.c), expected, "масштаб нажатия");
    assert_eq!(group.members[0].press.amount, 1.0);
}

#[test]
fn group_overflow_is_an_error_and_the_limit_itself_is_accepted() {
    let tokens = tokens();
    let node = placement(Point::new(100.0, 100.0), 48.0, 48.0);
    let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(GlassRole::Button), scale());
    for _ in 0..MAX_GLASS_GROUP_MEMBERS {
        builder
            .add_member(GlassShape::Circle, node, PressState::RELEASED)
            .unwrap();
    }
    let overflow = builder.add_member(GlassShape::Circle, node, PressState::RELEASED);
    assert_eq!(
        overflow,
        Err(ViewError::GlassGroupFull {
            max: MAX_GLASS_GROUP_MEMBERS
        })
    );
    // Отказ не испортил группу: ровно 16 членов, ничего не отброшено молча.
    let group = builder.build(1.0, Opacity::OPAQUE).unwrap();
    assert_eq!(group.members.len(), MAX_GLASS_GROUP_MEMBERS);
}

#[test]
fn merge_radius_is_converted_from_points_and_empty_or_bad_groups_are_errors() {
    let tokens = tokens();
    let style = tokens.glass.style_for(GlassRole::Button);
    let node = placement(Point::new(100.0, 100.0), 44.0, 44.0);
    let mut builder = GlassGroupBuilder::new(style, scale()).with_merge_radius_pt(12.0);
    builder
        .add_member(GlassShape::Circle, node, PressState::RELEASED)
        .unwrap();
    assert_eq!(
        builder.clone().build(1.0, Opacity::OPAQUE).unwrap().merge_radius_px,
        36.0
    );
    assert_eq!(builder.build(1.5, Opacity::OPAQUE), Err(ViewError::InvalidEffect(1.5)));
    let empty = GlassGroupBuilder::new(style, scale()).build(1.0, Opacity::OPAQUE);
    assert_eq!(empty, Err(ViewError::EmptyGlassGroup));
    assert_eq!(PxPerPt::new(0.0), Err(ViewError::InvalidScale(0.0)));
}

#[test]
fn member_transform_reports_a_missing_member() {
    let tokens = tokens();
    let node = placement(Point::new(100.0, 100.0), 48.0, 48.0);
    let mut builder = GlassGroupBuilder::new(tokens.glass.style_for(GlassRole::Button), scale());
    let first = builder
        .add_member(GlassShape::Circle, node, PressState::RELEASED)
        .unwrap();
    let group = builder.build(1.0, Opacity::OPAQUE).unwrap();
    assert_eq!(member_transform(&group, first).unwrap(), group.members[0].transform);
    let mut other = GlassGroupBuilder::new(tokens.glass.style_for(GlassRole::Button), scale());
    other
        .add_member(GlassShape::Circle, node, PressState::RELEASED)
        .unwrap();
    let second = other
        .add_member(GlassShape::Circle, node, PressState::RELEASED)
        .unwrap();
    assert_eq!(
        member_transform(&group, second),
        Err(ViewError::MemberOutOfRange { index: 1, len: 1 })
    );
}

// ---------- K2: содержимое и подписи ----------

#[test]
fn mode_label_cap_height_matches_the_measured_14_2_pt() {
    let tokens = tokens();
    let run = LabelPainter::new(&tokens.typography, scale())
        .layout("PHOTO", TextRoleKind::ModeLabel)
        .unwrap();
    // `layout.md` 2.1: заглавные 14,2 ± 0,5 pt.
    let cap_pt = run.cap_height_px / SCALE;
    assert!((cap_pt - 14.2).abs() < 0.5, "высота заглавных {cap_pt} pt");
    assert!(run.advance_px > 0.0);
}

#[test]
fn unsupported_character_is_an_explicit_error() {
    let tokens = tokens();
    let result = LabelPainter::new(&tokens.typography, scale()).layout("PH☃TO", TextRoleKind::ModeLabel);
    assert_eq!(
        result,
        Err(ViewError::Text(TextError::UnsupportedCharacter { ch: '☃', index: 2 }))
    );
}

#[test]
fn tabular_scale_value_has_the_same_width_for_different_digits() {
    let tokens = tokens();
    let painter = LabelPainter::new(&tokens.typography, scale());
    let one = painter.layout("1.0", TextRoleKind::ScaleValue).unwrap();
    let eight = painter.layout("8.8", TextRoleKind::ScaleValue).unwrap();
    assert_close(one.advance_px, eight.advance_px, "ширина табличных цифр");
}

#[test]
fn button_icon_is_centered_in_its_host_and_rotates_with_the_node() {
    let tokens = tokens();
    let colors = tokens.colors;
    let upright = node_placement_px(&visual(Point::new(100.0, 200.0), 48.0, 48.0, 0), f64::from(SCALE));
    let turned = node_placement_px(&visual(Point::new(100.0, 200.0), 48.0, 48.0, 1), f64::from(SCALE));
    let content = ButtonContent::Icon {
        icon: IconId::FlashOn,
        size_pt: 22.0,
    };
    let item = |placement: &NodePlacementPx| {
        let DrawItem::Icon(icon) = button_content_item(
            &content,
            scale(),
            placement.transform,
            colors.text_primary,
            Opacity::OPAQUE,
        ) else {
            panic!("ожидалась иконка");
        };
        icon
    };
    let (up, rot) = (item(&upright), item(&turned));
    assert_eq!(up.icon, IconId::FlashOn);
    assert_eq!(up.size_px, PxSize::new(66.0, 66.0));
    // Центр иконки совпадает с центром узла в обоих случаях.
    for icon in [&up, &rot] {
        let centre = centre_px(&icon.transform);
        assert_close(centre.x, 300.0, "центр x");
        assert_close(centre.y, 600.0, "центр y");
    }
    // Ось «вправо» иконки: у прямой — вправо, у повёрнутой на четверть — вниз (экранная ось y вниз).
    let right_up = up.transform.apply_point(PxPoint::new(10.0, 0.0));
    let right_rot = rot.transform.apply_point(PxPoint::new(10.0, 0.0));
    assert_close(right_up.x - 300.0, 10.0, "прямая: вправо");
    assert_close(right_up.y - 600.0, 0.0, "прямая: y");
    assert_close(right_rot.x - 300.0, 0.0, "повёрнутая: x");
    assert_close(right_rot.y - 600.0, 10.0, "повёрнутая: вниз");
}

#[test]
fn centered_label_sits_on_the_cap_height_middle_of_its_host() {
    let tokens = tokens();
    let run = LabelPainter::new(&tokens.typography, scale())
        .layout("PHOTO", TextRoleKind::ModeLabel)
        .unwrap();
    let host = placement(Point::new(220.0, 897.8), 106.0, 41.0);
    let DrawItem::Text(item) =
        ui_kit::view::centered_text(&run, host.transform, tokens.colors.text_primary, Opacity::OPAQUE)
    else {
        panic!("ожидался текст");
    };
    // Левый конец базовой линии: на половину ширины левее центра и на половину заглавных ниже.
    let origin = item.transform.apply_point(PxPoint::ORIGIN);
    assert_close(origin.x, 660.0 - run.advance_px / 2.0, "начало строки по x");
    assert_close(origin.y, 897.8 * 3.0 + run.cap_height_px / 2.0, "базовая линия по y");
}

// ---------- K8: плашка ----------

#[test]
fn banner_is_a_yellow_capsule_with_dark_text_and_follows_the_snapshot() {
    let tokens = tokens();
    let painter = LabelPainter::new(&tokens.typography, scale());
    let text = painter.layout("AE/AF LOCK", TextRoleKind::BannerText).unwrap();
    let size = measure_banner_pt(text.advance_px, scale(), &tokens.banner);
    assert_eq!(size.height, 30.0);
    assert!(size.width >= 117.0);

    let mut banner = Banner::new(&env(&tokens, MotionPreference::Full), ms(0)).unwrap();
    banner.show(LabelId(1), BannerLifetime::UntilHidden, ms(0)).unwrap();
    let node = placement(Point::new(220.0, 120.0), size.width, size.height);
    let mid = banner.snapshot(ms(60));
    let items = banner_items(&node, &mid, &text, &tokens.colors).unwrap();
    let [DrawItem::Shape(capsule), DrawItem::Text(label)] = items.as_slice() else {
        panic!("ожидались капсула и текст: {items:?}");
    };
    assert_eq!(capsule.fill, tokens.colors.focus_yellow);
    assert_eq!(label.color, tokens.colors.plate_text);
    assert_close(capsule.opacity.get(), mid.opacity, "прозрачность появления");
    // Масштаб появления запечён в преобразование: меньше полного размера.
    assert!(capsule.transform.a < 1.0 && capsule.transform.a > tokens.banner.appear_scale as f32 - 0.01);
    banner.advance(ms(3000)).unwrap();
    let full = banner_items(&node, &banner.snapshot(ms(3000)), &text, &tokens.colors).unwrap();
    let DrawItem::Shape(done) = &full[0] else { panic!() };
    assert_eq!((done.opacity.get(), done.transform.a), (1.0, 1.0));
}

#[test]
fn long_banner_text_widens_the_node_beyond_the_minimum() {
    let tokens = tokens();
    let painter = LabelPainter::new(&tokens.typography, scale());
    let short = painter.layout("AE", TextRoleKind::BannerText).unwrap();
    let long = painter
        .layout("AUTO EXPOSURE AND FOCUS LOCKED", TextRoleKind::BannerText)
        .unwrap();
    assert_eq!(
        measure_banner_pt(short.advance_px, scale(), &tokens.banner).width,
        117.0
    );
    assert!(measure_banner_pt(long.advance_px, scale(), &tokens.banner).width > 117.0);
}

// ---------- K9: рамка фокуса ----------

fn placed_frame(tokens: &UiKitTokens) -> FocusFrame<ui_kit::value::QuantizedLinear> {
    let mut frame = FocusFrame::new(
        exposure_model(),
        0.0,
        FocusFrameSpec::default(),
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    frame.place(Point::new(200.0, 400.0), ms(0)).unwrap();
    frame
}

#[test]
fn focus_frame_has_outline_four_inward_ticks_and_the_sun_to_the_right() {
    let tokens = tokens();
    let frame = placed_frame(&tokens);
    let snapshot = frame.snapshot(ms(2000));
    let node = placement(Point::new(200.0, 400.0), 91.0, 92.0);
    let items = focus_frame_items(&node, &snapshot, scale(), &tokens.focus, &tokens.colors).unwrap();
    assert_eq!(items.len(), 6, "контур + 4 тика + солнце");
    // Контур — квадрат размера узла с обводкой, без заливки.
    let DrawItem::Shape(outline) = &items[0] else { panic!() };
    assert_eq!(outline.geometry.extent(), PxSize::new(273.0, 276.0));
    assert!(outline.stroke.is_some() && outline.fill.a == 0.0);
    // Тики лежат внутри рамки, на серединах сторон, ближе к краю, чем к центру.
    let centre = PxPoint::new(600.0, 1200.0);
    for item in &items[1..5] {
        let DrawItem::Shape(tick) = item else { panic!() };
        let c = centre_px(&tick.transform);
        let (dx, dy) = ((c.x - centre.x).abs(), (c.y - centre.y).abs());
        assert!(dx <= 136.5 && dy <= 138.0, "тик внутри рамки");
        assert!(dx.max(dy) > 100.0 && dx.min(dy) < 0.01, "тик на середине стороны");
    }
    // Солнце: правее правой стороны на 21,5 pt и выше центра на 5 pt (layout.md 2.2).
    let DrawItem::Icon(sun) = &items[5] else { panic!() };
    assert_eq!(sun.icon, IconId::Exposure);
    let c = centre_px(&sun.transform);
    assert_close(c.x, centre.x + 136.5 + 21.5 * SCALE, "солнце x");
    assert_close(c.y, centre.y - 5.0 * SCALE, "солнце y");
}

#[test]
fn sun_follows_the_exposure_value_upwards() {
    let tokens = tokens();
    let frame = placed_frame(&tokens);
    let node = placement(Point::new(200.0, 400.0), 91.0, 92.0);
    let mut snapshot = frame.snapshot(ms(2000));
    let rest = focus_frame_items(&node, &snapshot, scale(), &tokens.focus, &tokens.colors).unwrap();
    snapshot.sun_travel_up_pt = 10.0;
    let raised = focus_frame_items(&node, &snapshot, scale(), &tokens.focus, &tokens.colors).unwrap();
    let (DrawItem::Icon(a), DrawItem::Icon(b)) = (&rest[5], &raised[5]) else {
        panic!()
    };
    assert_close(
        centre_px(&a.transform).y - centre_px(&b.transform).y,
        10.0 * SCALE,
        "ход солнца",
    );
}

// ---------- K10: затвор ----------

#[test]
fn shutter_is_a_glass_ring_under_a_white_disc_of_the_measured_ratio() {
    let tokens = tokens();
    let node = placement(Point::new(220.0, 801.9), 80.1, 80.1);
    let mut shutter = ShutterButton::new(
        Rect::centered_at(Point::new(220.0, 801.9), Size::new(80.1, 80.1)),
        ShutterLook::Ready,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    let items = shutter_items(
        &node,
        &shutter.snapshot(ms(0)),
        &tokens.glass.button,
        scale(),
        &tokens.shutter,
        &tokens.colors,
    )
    .unwrap();
    let [DrawItem::Glass(ring), DrawItem::Shape(disc)] = items.as_slice() else {
        panic!("ожидались стекло и диск: {items:?}");
    };
    // Порядок: стекло под диском. Кольцо — круг внешнего диаметра, диск — 67,9 / 80,1 от него.
    let ShapeGeometry::Circle {
        diameter: ring_diameter,
    } = ring.members[0].geometry
    else {
        panic!()
    };
    assert_close(ring_diameter, 80.1 * SCALE, "диаметр кольца");
    let ShapeGeometry::Circle { diameter } = disc.geometry else {
        panic!()
    };
    assert_close(diameter, 80.1 * SCALE * 67.9 / 80.1, "диаметр диска");
    assert_eq!(disc.fill, tokens.colors.text_primary);
    assert_eq!(disc.transform.a, 1.0);
    // Нажатие сжимает диск и увеличивает стекло.
    shutter
        .handle_press(
            ui_core::gesture::GestureState::Began,
            ui_core::gesture::GestureData::Press {
                position: Point::new(220.0, 801.9),
            },
            ms(0),
        )
        .unwrap();
    let pressed = shutter_items(
        &node,
        &shutter.snapshot(ms(2000)),
        &tokens.glass.button,
        scale(),
        &tokens.shutter,
        &tokens.colors,
    )
    .unwrap();
    let [DrawItem::Glass(ring), DrawItem::Shape(disc)] = pressed.as_slice() else {
        panic!()
    };
    assert_close(
        disc.transform.a,
        tokens.shutter.pressed_disc_scale as f32,
        "сжатие диска",
    );
    assert!(ring.members[0].transform.a > 1.0, "стекло увеличено при нажатии");
}

#[test]
fn disabled_and_busy_shutter_dim_by_tokens() {
    let tokens = tokens();
    let node = placement(Point::new(220.0, 801.9), 80.1, 80.1);
    let frame = Rect::centered_at(Point::new(220.0, 801.9), Size::new(80.1, 80.1));
    let build = |look| {
        let shutter = ShutterButton::new(frame, look, &env(&tokens, MotionPreference::Full), ms(0)).unwrap();
        let items = shutter_items(
            &node,
            &shutter.snapshot(ms(0)),
            &tokens.glass.button,
            scale(),
            &tokens.shutter,
            &tokens.colors,
        )
        .unwrap();
        let (DrawItem::Glass(ring), DrawItem::Shape(disc)) = (&items[0], &items[1]) else {
            panic!()
        };
        (ring.opacity.get(), disc.opacity.get())
    };
    assert_eq!(build(ShutterLook::Ready), (1.0, 1.0));
    let (ring, disc) = build(ShutterLook::Disabled);
    assert_close(ring, tokens.shutter.disabled_opacity as f32, "кольцо недоступного");
    assert_close(disc, tokens.shutter.disabled_opacity as f32, "диск недоступного");
    let (ring, disc) = build(ShutterLook::Busy);
    assert_eq!(ring, 1.0);
    assert_close(disc, tokens.shutter.busy_disc_opacity as f32, "диск занятого");
}

// ---------- K7: панель ----------

#[test]
fn panel_glass_follows_the_morph_frame_and_materialises_with_effect() {
    let tokens = tokens();
    let spec = PanelMorphSpec::new(
        Rect::from_xywh(358.0, 778.0, 48.0, 48.0),
        Rect::from_xywh(29.0, 340.0, 382.0, 416.0),
        40.0,
    );
    let mut panel = PanelMorph::new(spec, &env(&tokens, MotionPreference::Full), ms(0)).unwrap();
    let style = tokens.glass.style_for(GlassRole::Panel);
    let closed = panel_glass_group(&panel.snapshot(ms(0)), style, scale()).unwrap();
    assert_eq!(closed.effect, 0.0);
    assert_eq!(closed.members[0].geometry.extent(), PxSize::new(144.0, 144.0));

    panel.open(ms(0)).unwrap();
    let mid = panel.snapshot(ms(150));
    let group = panel_glass_group(&mid, style, scale()).unwrap();
    assert_eq!(group.effect, mid.effect);
    assert_close(
        group.members[0].geometry.extent().width,
        mid.frame.size.width as f32 * SCALE,
        "ширина",
    );
    let centre = centre_px(&group.members[0].transform);
    assert_close(centre.x, mid.frame.center().x as f32 * SCALE, "центр x");
    assert_close(centre.y, mid.frame.center().y as f32 * SCALE, "центр y");

    let open = panel_glass_group(&panel.snapshot(ms(3000)), style, scale()).unwrap();
    assert_eq!(open.effect, 1.0);
    assert_eq!(
        open.members[0].geometry.extent(),
        PxSize::new(382.0 * SCALE, 416.0 * SCALE)
    );
}

// ---------- Всё вместе: экран из одних строителей ----------

#[test]
fn a_whole_camera_screen_built_from_kit_builders_passes_draw_list_validation() {
    let tokens = tokens();
    let painter = LabelPainter::new(&tokens.typography, scale());
    let mut items: Vec<DrawItem> = Vec::new();

    // 1. Не стекло под стеклом: плашка и рамка фокуса.
    let text = painter.layout("AE/AF LOCK", TextRoleKind::BannerText).unwrap();
    let banner_size = measure_banner_pt(text.advance_px, scale(), &tokens.banner);
    let mut banner = Banner::new(&env(&tokens, MotionPreference::Full), ms(0)).unwrap();
    banner.show(LabelId(1), BannerLifetime::UntilHidden, ms(0)).unwrap();
    let banner_node = placement(Point::new(220.0, 120.0), banner_size.width, banner_size.height);
    items.extend(banner_items(&banner_node, &banner.snapshot(ms(500)), &text, &tokens.colors).unwrap());
    let frame = placed_frame(&tokens);
    let focus_node = placement(Point::new(200.0, 400.0), 91.0, 92.0);
    items.extend(
        focus_frame_items(
            &focus_node,
            &frame.snapshot(ms(500)),
            scale(),
            &tokens.focus,
            &tokens.colors,
        )
        .unwrap(),
    );

    // 2. Стекло: капсула режимов + две круглые кнопки; 3. содержимое поверх.
    let style = tokens.glass.style_for(GlassRole::Button);
    let mut group = GlassGroupBuilder::new(style, scale());
    let capsule = group
        .add_member(
            GlassShape::Capsule,
            placement(Point::new(220.0, 897.8), 220.0, 47.8),
            PressState::RELEASED,
        )
        .unwrap();
    let flash = group
        .add_member(
            GlassShape::Circle,
            placement(Point::new(60.0, 800.0), 48.0, 48.0),
            PressState::RELEASED,
        )
        .unwrap();
    let group = group.build(1.0, Opacity::OPAQUE).unwrap();
    let mode = painter.layout("PHOTO", TextRoleKind::ModeLabel).unwrap();
    let capsule_host = member_transform(&group, capsule).unwrap();
    let flash_host = member_transform(&group, flash).unwrap();
    items.push(DrawItem::Glass(group));
    items.push(ui_kit::view::centered_text(
        &mode,
        capsule_host,
        tokens.colors.text_primary,
        Opacity::OPAQUE,
    ));
    items.push(centered_icon(
        IconId::FlashOn,
        22.0,
        scale(),
        flash_host,
        tokens.colors.text_primary,
        Opacity::OPAQUE,
    ));

    // Затвор и панель.
    let shutter = ShutterButton::new(
        Rect::centered_at(Point::new(220.0, 801.9), Size::new(80.1, 80.1)),
        ShutterLook::Ready,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    items.extend(
        shutter_items(
            &placement(Point::new(220.0, 801.9), 80.1, 80.1),
            &shutter.snapshot(ms(0)),
            style,
            scale(),
            &tokens.shutter,
            &tokens.colors,
        )
        .unwrap(),
    );
    let mut panel = PanelMorph::new(
        PanelMorphSpec::new(
            Rect::from_xywh(358.0, 778.0, 48.0, 48.0),
            Rect::from_xywh(29.0, 340.0, 382.0, 416.0),
            40.0,
        ),
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    panel.open(ms(0)).unwrap();
    items.push(DrawItem::Glass(
        panel_glass_group(
            &panel.snapshot(ms(200)),
            tokens.glass.style_for(GlassRole::Panel),
            scale(),
        )
        .unwrap(),
    ));

    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    list.items = items;
    list.validate().unwrap();
    // Каждый элемент что-то закрашивает: границы определены.
    assert!(list.items.iter().all(|item| item.bounds_px().is_some()));
}
