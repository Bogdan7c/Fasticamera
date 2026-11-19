#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Эталон целых элементов: фигура (`shade_shape_pixel`), тень, группа стекла с тенью и правило
//! нажатия группы, параметры SDF. Это формулы, которые исполняют шейдеры `ui_shape` и `ui_glass`.

use ui_draw::color::{DisplayColor, Opacity, PremultipliedColor};
use ui_draw::draw_list::{
    GlassGroupItem, GlassMember, PressState, ShapeItem, StrokeStyle, shade_glass_group_pixel, shade_shape_pixel,
};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::reference::{
    GlassPixelInput, MATERIALIZE_LAYER_RAMP, PressInput, shade_glass_pixel, shade_glass_shadow, shade_glass_with_shadow,
};
use ui_draw::glass::{GlassStyle, ShadowParamsPx};
use ui_draw::light::LightDirection;
use ui_draw::shape::{CornerSpec, ShapeGeometry, shadow_falloff, shadow_probe_point};
use ui_draw::transform::Affine2;

const AA: f32 = 1.0;

fn shadow(radius_px: f32, alpha: f32, offset_px: f32) -> ShadowParamsPx {
    ShadowParamsPx {
        radius_px,
        alpha,
        offset_px,
    }
}

fn shape(geometry: ShapeGeometry, transform: Affine2, fill: DisplayColor) -> ShapeItem {
    ShapeItem {
        geometry,
        transform,
        fill,
        stroke: None,
        shadow: None,
        opacity: Opacity::OPAQUE,
    }
}

fn shade(item: &ShapeItem, x: f32, y: f32) -> PremultipliedColor {
    shade_shape_pixel(item, PxPoint::new(x, y), AA).unwrap()
}

#[test]
fn shadow_falloff_is_half_on_the_border_full_inside_and_zero_outside() {
    assert!((shadow_falloff(0.0, 10.0, AA) - 0.5).abs() < 1e-6);
    assert_eq!(shadow_falloff(-10.0, 10.0, AA), 1.0);
    assert_eq!(shadow_falloff(10.0, 10.0, AA), 0.0);
    // Радиус 0 — резкая копия формы со сглаживанием края шириной aa: центр края 0,5, внутри 1.
    assert!((shadow_falloff(0.0, 0.0, AA) - 0.5).abs() < 1e-6);
    assert_eq!(shadow_falloff(-2.0, 0.0, AA), 1.0);
    assert_eq!(shadow_falloff(2.0, 0.0, AA), 0.0);
}

#[test]
fn shadow_probe_point_looks_up_by_the_offset() {
    let probe = shadow_probe_point(PxPoint::new(30.0, 50.0), 6.0);
    assert_eq!((probe.x, probe.y), (30.0, 44.0));
}

#[test]
fn shape_shadow_is_below_the_shape_and_absent_above_it() {
    let mut item = shape(
        ShapeGeometry::Circle { diameter: 40.0 },
        Affine2::translate(100.0, 100.0),
        DisplayColor::rgb(1.0, 0.0, 0.0),
    );
    item.shadow = Some(shadow(6.0, 0.6, 6.0));
    // Низ круга y = 120; сдвинутая форма кончается на 126. Точка y = 123: расстояние −3 → t = 0,75 →
    // smoothstep 0,84375 · 0,6.
    let below = shade(&item, 100.0, 123.0);
    assert!((below.a - 0.84375 * 0.6).abs() < 1e-4, "тень снизу: {below:?}");
    assert_eq!((below.r, below.g, below.b), (0.0, 0.0, 0.0), "тень чёрная");
    // Верх круга y = 80; сдвинутая форма начинается с 86. Точка y = 77 — дальше радиуса тени: пусто.
    assert_eq!(shade(&item, 100.0, 77.0), PremultipliedColor::TRANSPARENT);
    // Под заливкой тень не видна: внутри круга цвет — чистая заливка.
    let inside = shade(&item, 100.0, 100.0);
    assert_eq!((inside.r, inside.g, inside.b, inside.a), (1.0, 0.0, 0.0, 1.0));
}

#[test]
fn shape_without_shadow_is_transparent_outside() {
    let item = shape(
        ShapeGeometry::Circle { diameter: 40.0 },
        Affine2::translate(100.0, 100.0),
        DisplayColor::WHITE,
    );
    assert_eq!(shade(&item, 100.0, 123.0), PremultipliedColor::TRANSPARENT);
}

#[test]
fn stroke_is_centered_on_the_border_in_local_pixels() {
    // Квадрат 40×40 без скругления, правая граница x = 70, прозрачная заливка, обводка 4 px.
    let mut item = shape(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(40.0, 40.0),
            corner: CornerSpec::circular(0.0),
        },
        Affine2::translate(50.0, 50.0),
        DisplayColor::TRANSPARENT,
    );
    item.stroke = Some(StrokeStyle {
        width_px: 4.0,
        color: DisplayColor::WHITE,
    });
    // Края обводки: 68 (внутри) и 72 (снаружи) — покрытие 0,5 с обеих сторон.
    assert!((shade(&item, 68.0, 50.0).a - 0.5).abs() < 1e-5);
    assert!((shade(&item, 72.0, 50.0).a - 0.5).abs() < 1e-5);
    // На самой границе и рядом — полное; за пределами полосы — ноль.
    assert_eq!(shade(&item, 70.0, 50.0).a, 1.0);
    assert_eq!(shade(&item, 66.0, 50.0).a, 0.0);
    assert_eq!(shade(&item, 74.0, 50.0).a, 0.0);
}

#[test]
fn stroke_width_is_local_so_a_scaled_shape_gets_a_proportionally_wider_stroke() {
    // Тот же квадрат, масштаб 2: граница x = 90 (центр 50, полуширина 40), обводка 4 локальных = 8 px.
    let mut item = shape(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(40.0, 40.0),
            corner: CornerSpec::circular(0.0),
        },
        Affine2::uniform_scale(2.0).then(Affine2::translate(50.0, 50.0)),
        DisplayColor::TRANSPARENT,
    );
    item.stroke = Some(StrokeStyle {
        width_px: 4.0,
        color: DisplayColor::WHITE,
    });
    assert_eq!(shade(&item, 93.0, 50.0).a, 1.0, "3 px снаружи — внутри полосы ±4");
    assert_eq!(shade(&item, 95.0, 50.0).a, 0.0, "5 px снаружи — уже за полосой");
}

#[test]
fn opacity_scales_fill_stroke_and_shadow_together() {
    let mut item = shape(
        ShapeGeometry::Circle { diameter: 40.0 },
        Affine2::translate(100.0, 100.0),
        DisplayColor::rgb(1.0, 0.5, 0.0),
    );
    item.shadow = Some(shadow(6.0, 0.6, 6.0));
    let full_inside = shade(&item, 100.0, 100.0);
    let full_shadow = shade(&item, 100.0, 123.0);
    item.opacity = Opacity::new(0.5).unwrap();
    let half_inside = shade(&item, 100.0, 100.0);
    let half_shadow = shade(&item, 100.0, 123.0);
    assert!((half_inside.a - 0.5 * full_inside.a).abs() < 1e-6);
    assert!((half_inside.g - 0.5 * full_inside.g).abs() < 1e-6);
    assert!((half_shadow.a - 0.5 * full_shadow.a).abs() < 1e-6);
}

#[test]
fn degenerate_shape_transform_is_an_error_not_a_pixel() {
    let item = shape(
        ShapeGeometry::Circle { diameter: 10.0 },
        Affine2::scale(0.0, 1.0),
        DisplayColor::WHITE,
    );
    assert!(shade_shape_pixel(&item, PxPoint::new(0.0, 0.0), AA).is_err());
}

#[test]
fn sdf_parameters_describe_each_geometry() {
    let circle = ShapeGeometry::Circle { diameter: 40.0 }.sdf_parameters();
    assert_eq!((circle.half_size.width, circle.half_size.height), (20.0, 20.0));
    assert_eq!((circle.radius, circle.exponent), (20.0, 2.0));
    let capsule = ShapeGeometry::Capsule {
        size: PxSize::new(100.0, 40.0),
    }
    .sdf_parameters();
    assert_eq!((capsule.half_size.width, capsule.half_size.height), (50.0, 20.0));
    assert_eq!((capsule.radius, capsule.exponent), (20.0, 2.0));
    // Радиус больше половины меньшей стороны ограничивается; непрерывный угол несёт свой показатель.
    let rect = ShapeGeometry::RoundedRect {
        size: PxSize::new(100.0, 40.0),
        corner: CornerSpec::continuous(90.0),
    }
    .sdf_parameters();
    assert_eq!((rect.radius, rect.exponent), (20.0, 5.0));
    let arc = ShapeGeometry::RoundedRect {
        size: PxSize::new(100.0, 40.0),
        corner: CornerSpec::circular(8.0),
    }
    .sdf_parameters();
    assert_eq!((arc.radius, arc.exponent), (8.0, 2.0));
}

fn glass_member(center: PxPoint, press_amount: f32, touch_local: PxPoint) -> GlassMember {
    GlassMember {
        geometry: ShapeGeometry::Circle { diameter: 60.0 },
        transform: Affine2::translate(center.x, center.y),
        press: PressState {
            amount: press_amount,
            touch_point_local: touch_local,
        },
    }
}

fn group_of(members: Vec<GlassMember>) -> GlassGroupItem {
    GlassGroupItem {
        params: GlassStyle::default().resolve(PxSize::new(60.0, 60.0), 3.0),
        members,
        merge_radius_px: 0.0,
        effect: 1.0,
        opacity: Opacity::OPAQUE,
    }
}

#[test]
fn dominant_press_picks_the_strongest_member_and_the_first_on_a_tie() {
    let group = group_of(vec![
        glass_member(PxPoint::new(50.0, 50.0), 0.2, PxPoint::new(1.0, 1.0)),
        glass_member(PxPoint::new(200.0, 50.0), 0.7, PxPoint::new(5.0, -3.0)),
        glass_member(PxPoint::new(350.0, 50.0), 0.7, PxPoint::new(9.0, 9.0)),
    ]);
    let press = group.dominant_press();
    assert_eq!(press.amount, 0.7);
    // Точка касания второго члена в экранных px: центр (200, 50) + (5, −3).
    assert_eq!((press.touch_point_px.x, press.touch_point_px.y), (205.0, 47.0));
}

#[test]
fn dominant_press_without_press_or_members_is_none_or_first_member() {
    assert_eq!(group_of(Vec::new()).dominant_press(), PressInput::NONE);
    let released = group_of(vec![
        glass_member(PxPoint::new(50.0, 50.0), 0.0, PxPoint::new(2.0, 0.0)),
        glass_member(PxPoint::new(200.0, 50.0), 0.0, PxPoint::new(7.0, 0.0)),
    ]);
    let press = released.dominant_press();
    assert_eq!(press.amount, 0.0);
    assert_eq!(press.touch_point_px.x, 52.0, "при равенстве нулей — первый член");
}

fn input<'a>(group: &'a GlassGroupItem, sd_px: f32, effect: f32) -> GlassPixelInput<'a> {
    GlassPixelInput {
        screen_pos: PxPoint::new(0.0, 0.0),
        group_sd_px: sd_px,
        normal_out: ui_draw::geometry::PxVector::new(0.0, 1.0),
        params: &group.params,
        light: LightDirection::from_angle_degrees(30.0),
        backdrop_mean_luma: 0.5,
        press: PressInput::NONE,
        effect,
        opacity: Opacity::OPAQUE,
        aa_width_px: AA,
    }
}

fn gray(_: PxPoint) -> DisplayColor {
    DisplayColor::rgb(0.4, 0.4, 0.4)
}

#[test]
fn glass_shadow_scales_with_effect_opacity_and_alpha_and_is_black() {
    let params = shadow(8.0, 0.5, 6.0);
    // Расстояние 0 → falloff 0,5.
    let base = shade_glass_shadow(0.0, &params, 1.0, Opacity::OPAQUE, AA);
    assert!((base.a - 0.25).abs() < 1e-6);
    assert_eq!((base.r, base.g, base.b), (0.0, 0.0, 0.0));
    let half_effect = shade_glass_shadow(0.0, &params, 0.5, Opacity::OPAQUE, AA);
    assert!((half_effect.a - 0.125).abs() < 1e-6);
    let half_opacity = shade_glass_shadow(0.0, &params, 1.0, Opacity::new(0.5).unwrap(), AA);
    assert!((half_opacity.a - 0.125).abs() < 1e-6);
    let none = shade_glass_shadow(-100.0, &shadow(8.0, 0.0, 6.0), 1.0, Opacity::OPAQUE, AA);
    assert_eq!(none, PremultipliedColor::TRANSPARENT);
    let hidden = shade_glass_shadow(-100.0, &params, 0.0, Opacity::OPAQUE, AA);
    assert_eq!(hidden.a, 0.0, "effect = 0 — тени нет");
}

#[test]
fn glass_pixel_alone_is_empty_outside_but_with_shadow_it_shows_the_shadow() {
    let group = group_of(vec![glass_member(PxPoint::new(0.0, 0.0), 0.0, PxPoint::ORIGIN)]);
    // Снаружи формы на 3 px: стекла нет, а сдвинутая вниз форма уже накрывает пиксель (−2).
    let outside = input(&group, 3.0, 1.0);
    assert_eq!(shade_glass_pixel(gray, &outside), PremultipliedColor::TRANSPARENT);
    let with_shadow = shade_glass_with_shadow(gray, &outside, -2.0);
    assert!(
        with_shadow.a > 0.0 && with_shadow.r == 0.0,
        "тень без стекла: {with_shadow:?}"
    );
    // Внутри формы стекло непрозрачно и закрывает тень целиком.
    let inside = input(&group, -20.0, 1.0);
    let glass = shade_glass_pixel(gray, &inside);
    let covered = shade_glass_with_shadow(gray, &inside, -20.0);
    assert_eq!(glass.a, 1.0);
    assert_eq!(covered, glass);
}

#[test]
fn group_pixel_has_a_shadow_below_the_group_and_none_above_it() {
    let group = group_of(vec![glass_member(PxPoint::new(100.0, 100.0), 0.0, PxPoint::ORIGIN)]);
    let offset = group.params.shadow.offset_px;
    let radius = group.params.shadow.radius_px;
    assert!(offset > 0.0 && radius > 0.0, "стиль по умолчанию задаёт тень");
    let light = LightDirection::from_angle_degrees(30.0);
    let pixel = |y: f32| shade_glass_group_pixel(&group, light, PxPoint::new(100.0, y), 0.5, gray, AA).unwrap();
    // Круг радиуса 30: низ y = 130, верх y = 70. Берём точки на 2 px снаружи.
    let below = pixel(132.0);
    let above = pixel(68.0);
    // Обе точки на 2 px снаружи формы; тень сдвинута вниз, поэтому снизу пиксель на `offset` ближе к ней,
    // сверху — на `offset` дальше. Ожидание считается по формуле, а не подгоняется под результат.
    let alpha = group.params.shadow.alpha;
    let expected_below = shadow_falloff(2.0 - offset, radius, AA) * alpha;
    let expected_above = shadow_falloff(2.0 + offset, radius, AA) * alpha;
    assert!(
        (below.a - expected_below).abs() < 1e-4,
        "{} против {expected_below}",
        below.a
    );
    assert!(
        (above.a - expected_above).abs() < 1e-4,
        "{} против {expected_above}",
        above.a
    );
    assert!(
        below.a > above.a + 0.01,
        "снизу тени заметно больше: {} против {}",
        below.a,
        above.a
    );
    assert_eq!((below.r, below.g, below.b), (0.0, 0.0, 0.0));
    // Внутри — стекло.
    assert_eq!(pixel(100.0).a, 1.0);
}

#[test]
fn group_pixel_of_an_empty_group_is_transparent_and_a_degenerate_member_is_an_error() {
    let light = LightDirection::from_angle_degrees(30.0);
    let empty = group_of(Vec::new());
    assert_eq!(
        shade_glass_group_pixel(&empty, light, PxPoint::new(1.0, 1.0), 0.5, gray, AA).unwrap(),
        PremultipliedColor::TRANSPARENT
    );
    let mut broken = group_of(vec![glass_member(PxPoint::new(0.0, 0.0), 0.0, PxPoint::ORIGIN)]);
    broken.members[0].transform = Affine2::scale(0.0, 0.0);
    assert!(shade_glass_group_pixel(&broken, light, PxPoint::new(1.0, 1.0), 0.5, gray, AA).is_err());
}

#[test]
fn group_pixel_uses_the_dominant_press_for_glow() {
    // Нажатый второй член с касанием: свечение белит пиксель у точки касания сильнее, чем без нажатия.
    let mut group = group_of(vec![
        glass_member(PxPoint::new(100.0, 100.0), 0.0, PxPoint::ORIGIN),
        glass_member(PxPoint::new(180.0, 100.0), 1.0, PxPoint::new(0.0, 0.0)),
    ]);
    group.params.press.glow_alpha = 0.8;
    group.params.press.glow_radius_px = 60.0;
    let light = LightDirection::from_angle_degrees(30.0);
    let probe = PxPoint::new(180.0, 100.0);
    let pressed = shade_glass_group_pixel(&group, light, probe, 0.5, gray, AA).unwrap();
    group.members[1].press.amount = 0.0;
    let released = shade_glass_group_pixel(&group, light, probe, 0.5, gray, AA).unwrap();
    assert!(
        pressed.r > released.r + 0.2,
        "свечение нажатия: {pressed:?} против {released:?}"
    );
}

#[test]
fn materialize_ramp_fades_the_layer_in_over_the_first_part_of_effect() {
    let group = group_of(vec![glass_member(PxPoint::new(0.0, 0.0), 0.0, PxPoint::ORIGIN)]);
    let alpha_at = |effect: f32| shade_glass_pixel(gray, &input(&group, -20.0, effect)).a;
    assert_eq!(alpha_at(0.0), 0.0);
    // Середина рампы: smoothstep(0,1,0,5) = 0,5.
    assert!((alpha_at(MATERIALIZE_LAYER_RAMP * 0.5) - 0.5).abs() < 1e-5);
    assert!((alpha_at(MATERIALIZE_LAYER_RAMP) - 1.0).abs() < 1e-6);
    assert_eq!(alpha_at(1.0), 1.0);
}
