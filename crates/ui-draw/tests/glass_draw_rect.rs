//! Квад рисования стекла (`GlassGroupItem::draw_rect_px`) против прямоугольника адаптации яркости
//! (`adaptation_rect_px`): первый узкий и содержит только то, что может дать вклад; второй (с решения 220) — границы форм членов без запасов,
//! а консервативные границы элемента (`bounds_px`) по-прежнему раздуты запасами.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use ui_draw::color::Opacity;
use ui_draw::draw_list::{DrawItem, GlassGroupItem, GlassMember, PressState};
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::{GlassStyle, ShadowParamsPx};
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

const PX_PER_PT: f32 = 3.0;

fn member(geometry: ShapeGeometry, transform: Affine2) -> GlassMember {
    GlassMember {
        geometry,
        transform,
        press: PressState::RELEASED,
    }
}

/// Одна форма 200 × 100 в точке (300, 300), стиль по умолчанию, без слияния.
fn rect_group() -> GlassGroupItem {
    let geometry = ShapeGeometry::RoundedRect {
        size: PxSize::new(200.0, 100.0),
        corner: ui_draw::shape::CornerSpec::continuous(24.0),
    };
    GlassGroupItem {
        params: GlassStyle::default().resolve(geometry.extent(), PX_PER_PT),
        members: vec![member(geometry, Affine2::translate(300.0, 300.0))],
        merge_radius_px: 0.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

fn shadow(radius_px: f32, alpha: f32, offset_px: f32) -> ShadowParamsPx {
    ShadowParamsPx {
        radius_px,
        alpha,
        offset_px,
    }
}

#[test]
fn glow_and_refraction_do_not_widen_the_draw_rect_or_the_adaptation_rect_but_do_widen_the_bounds() {
    let mut group = rect_group();
    group.params.shadow = shadow(10.0, 0.5, 4.0);
    let baseline = group.draw_rect_px().unwrap();
    let adaptation_baseline = group.adaptation_rect_px().unwrap();
    // Свечение и вынос линзы сколь угодно велики: рисовать они всё равно не могут (смешиваются внутри слоя / меняют точку чтения фона).
    group.params.press.glow_radius_px = 5000.0;
    group.params.refraction_strength_px = 3000.0;
    assert_eq!(
        group.draw_rect_px().unwrap(),
        baseline,
        "квад рисования не зависит от свечения и линзы"
    );
    assert_eq!(
        group.adaptation_rect_px().unwrap(),
        adaptation_baseline,
        "прямоугольник адаптации не зависит от свечения и линзы"
    );
    let bounds = DrawItem::Glass(group.clone()).bounds_px().unwrap();
    assert!(
        bounds.size.width > 9000.0,
        "границы элемента по-прежнему раздуты свечением"
    );
    assert_eq!(
        bounds,
        group.influence_rect_px().unwrap(),
        "общие границы элемента — консервативные границы группы"
    );
}

#[test]
fn adaptation_rect_is_the_union_of_member_shapes_without_margins() {
    // Форма 200 x 100 в (300, 300) и круг 60 справа; тень, слияние и свечение заданы большими: на прямоугольник не влияют.
    let mut group = rect_group();
    group.members.push(member(
        ShapeGeometry::Circle { diameter: 60.0 },
        Affine2::translate(600.0, 320.0),
    ));
    group.merge_radius_px = 80.0;
    group.params.shadow = shadow(40.0, 0.5, 12.0);
    group.params.press.glow_radius_px = 70.0;
    // Члены: x 200..400, y 250..350 и x 570..630, y 290..350 -> объединение x 200..630, y 250..350.
    assert_eq!(
        group.adaptation_rect_px().unwrap(),
        PxRect::new(PxPoint::new(200.0, 250.0), PxSize::new(430.0, 100.0))
    );
    let bounds = group.bounds_px().unwrap();
    assert!(
        bounds.size.width > 430.0 + 100.0 && bounds.size.height > 100.0 + 100.0,
        "границы элемента раздуты запасом: {bounds:?}"
    );
}

#[test]
fn shadow_extends_the_draw_rect_down_more_than_up() {
    let mut group = rect_group();
    group.params.shadow = shadow(30.0, 0.5, 10.0);
    let with_shadow = group.draw_rect_px().unwrap();
    group.params.shadow.alpha = 0.0;
    let without_shadow = group.draw_rect_px().unwrap();
    // Форма 200 × 100 в (300, 300): x 200..400, y 250..350; запас сглаживания 1 px.
    assert_eq!(
        without_shadow,
        PxRect::new(PxPoint::new(199.0, 249.0), PxSize::new(202.0, 102.0))
    );
    assert!(
        (with_shadow.origin.x - (200.0 - 31.0)).abs() < 1e-3,
        "слева радиус тени и сглаживание"
    );
    assert!(
        (with_shadow.origin.y - (250.0 - 31.0 + 10.0)).abs() < 1e-3,
        "сверху радиус минус сдвиг"
    );
    assert!(
        (with_shadow.max_y() - (350.0 + 31.0 + 10.0)).abs() < 1e-3,
        "снизу радиус плюс сдвиг"
    );
}

#[test]
fn merge_radius_adds_a_quarter_of_itself() {
    let mut group = rect_group();
    group.params.shadow.alpha = 0.0;
    let plain = group.draw_rect_px().unwrap();
    group.merge_radius_px = 40.0;
    let merged = group.draw_rect_px().unwrap();
    assert!(
        (plain.origin.x - merged.origin.x - 10.0).abs() < 1e-3,
        "выпуклость слияния: 40 / 4"
    );
}

#[test]
fn stretched_member_needs_a_wider_reach_than_a_uniform_one() {
    // Растяжение 2:1 по x: расстояние поля делится на длину градиента, поэтому тень уходит дальше по x.
    let mut group = rect_group();
    group.params.shadow = shadow(20.0, 0.5, 0.0);
    let uniform = group.draw_rect_px().unwrap();
    let geometry = group.members[0].geometry;
    group.members[0].transform = Affine2::scale(2.0, 1.0).then(Affine2::translate(300.0, 300.0));
    let stretched = group.draw_rect_px().unwrap();
    let uniform_margin_x = 200.0 - uniform.origin.x;
    let shape_left = 300.0 - geometry.extent().width;
    let stretched_margin_x = shape_left - stretched.origin.x;
    assert!(uniform_margin_x > 20.0);
    assert!(
        stretched_margin_x > uniform_margin_x * 1.9,
        "по растянутой оси запас ≈ в σmax/σmin раз больше: {stretched_margin_x} против {uniform_margin_x}"
    );
}

#[test]
fn empty_group_has_no_draw_rect() {
    let mut group = rect_group();
    group.members.clear();
    assert_eq!(group.draw_rect_px(), None);
    assert_eq!(group.adaptation_rect_px(), None);
    assert_eq!(group.bounds_px(), None);
}

#[test]
fn degenerate_member_has_no_draw_rect() {
    let mut group = rect_group();
    group.members[0].transform = Affine2::scale(0.0, 1.0);
    assert_eq!(group.draw_rect_px(), None);
}

#[test]
fn non_finite_transform_has_no_adaptation_rect_and_no_bounds() {
    let mut group = rect_group();
    group.members[0].transform = Affine2::translate(f32::NAN, 0.0);
    assert_eq!(group.adaptation_rect_px(), None);
    assert_eq!(group.bounds_px(), None);
}
