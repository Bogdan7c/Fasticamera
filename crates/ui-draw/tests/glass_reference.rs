#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Эталонные формулы стекла: линза, дисперсия, тон, блик, композиция пикселя.

use ui_draw::color::{DisplayColor, Opacity, luma_rec709};
use ui_draw::geometry::{PxPoint, PxSize, PxVector};
use ui_draw::glass::reference::{
    GlassPixelInput, PressInput, backdrop_dimming, dark_edge, lens_profile, press_glow, refraction_offsets,
    shade_glass_pixel, specular_intensity, vibrancy,
};
use ui_draw::glass::{EdgeProfile, GlassParamsPx, GlassStyle};
use ui_draw::light::LightDirection;
use ui_draw::shape::{ShapeGeometry, corrected_distance};

const PX_PER_PT: f32 = 2.4545;
/// Кнопка 44 pt: меньшая сторона ≈ 108 px.
const BUTTON_SIDE: f32 = 108.0;

fn button_params() -> GlassParamsPx {
    GlassStyle::default().resolve(PxSize::new(BUTTON_SIDE * 2.0, BUTTON_SIDE), PX_PER_PT)
}

#[test]
fn lens_profile_is_one_at_the_edge_zero_at_the_zone_end_and_monotonic() {
    for profile in [EdgeProfile::QuarterCircle, EdgeProfile::ConvexSuperellipse] {
        assert!(
            (lens_profile(0.0, 27.0, profile) - 1.0).abs() < 1e-6,
            "{profile:?}: край"
        );
        assert_eq!(lens_profile(27.0, 27.0, profile), 0.0, "{profile:?}: конец зоны");
        assert_eq!(lens_profile(90.0, 27.0, profile), 0.0, "{profile:?}: глубже зоны");
        let mut previous = f32::INFINITY;
        for step in 0..=27 {
            let value = lens_profile(step as f32, 27.0, profile);
            assert!(value <= previous + 1e-6, "{profile:?}: не монотонно на {step}");
            previous = value;
        }
    }
    // Известные значения: u = 0,5 → окружность 1−√0,75 = 0,134, суперэллипс 1−(1−1/16)^(1/4) = 0,0159.
    let circle = lens_profile(13.5, 27.0, EdgeProfile::QuarterCircle);
    let superellipse = lens_profile(13.5, 27.0, EdgeProfile::ConvexSuperellipse);
    assert!((circle - 0.1340).abs() < 1e-3, "{circle}");
    assert!((superellipse - 0.0160).abs() < 1e-3, "{superellipse}");
    assert_eq!(
        lens_profile(5.0, 0.0, EdgeProfile::QuarterCircle),
        0.0,
        "нулевая зона — линзы нет"
    );
}

#[test]
fn refraction_is_zero_in_the_center_and_full_at_the_edge_pointing_outward() {
    let params = button_params();
    let capsule = ShapeGeometry::Capsule {
        size: PxSize::new(BUTTON_SIDE * 2.0, BUTTON_SIDE),
    };
    // Центр формы глубже зоны линзы (27 px): смещение 0.
    let center = corrected_distance(&capsule, PxPoint::new(30.0, 0.0));
    let center_offsets = refraction_offsets(center.distance, center.normal_out, &params, 0.0);
    assert!(
        center_offsets.g.length() < 1e-3,
        "в центре смещение {:?}",
        center_offsets.g
    );
    // Самый край у правого кончика: нормаль наружу +x, смещение = сила 54 px (50 % высоты).
    let edge = corrected_distance(&capsule, PxPoint::new(BUTTON_SIDE, 0.0));
    let edge_offsets = refraction_offsets(edge.distance, edge.normal_out, &params, 0.0);
    assert!(edge_offsets.g.length() >= 0.9 * params.refraction_strength_px);
    assert!(
        edge_offsets.g.dx > 0.0 && edge_offsets.g.dy.abs() < 1e-3,
        "выборка должна идти наружу: {:?}",
        edge_offsets.g
    );
}

#[test]
fn dispersion_separates_red_and_blue_by_two_delta_times_the_offset() {
    let params = button_params();
    let normal = PxVector::new(0.0, -1.0);
    let offsets = refraction_offsets(0.0, normal, &params, 0.0);
    let green = offsets.g.length();
    assert!(green > 1.0);
    let separation = (offsets.b - offsets.r).length();
    assert!(
        (separation - 2.0 * params.dispersion * green).abs() < 1e-3,
        "{separation}"
    );
    // R короче зелёного, B длиннее.
    assert!(offsets.r.length() < green && offsets.b.length() > green);
    // Без дисперсии каналы совпадают.
    let flat = GlassParamsPx {
        dispersion: 0.0,
        ..params
    };
    let flat_offsets = refraction_offsets(0.0, normal, &flat, 0.0);
    assert_eq!(flat_offsets.r, flat_offsets.b);
}

#[test]
fn pressing_boosts_refraction() {
    let params = button_params();
    let normal = PxVector::new(1.0, 0.0);
    let released = refraction_offsets(0.0, normal, &params, 0.0).g.length();
    let pressed = refraction_offsets(0.0, normal, &params, 1.0).g.length();
    assert!((pressed / released - (1.0 + params.press.refraction_boost)).abs() < 1e-4);
}

#[test]
fn dimming_is_monotonic_capped_and_stronger_over_bright_backdrop() {
    let params = button_params();
    let dark = backdrop_dimming(0.1, &params.dimming);
    let bright = backdrop_dimming(0.95, &params.dimming);
    assert_eq!(dark, 0.0);
    assert!(
        (bright - 0.35).abs() < 1e-6,
        "над ярким фоном — потолок HIG 35 %: {bright}"
    );
    let mut previous = -1.0_f32;
    for step in 0..=20 {
        let value = backdrop_dimming(step as f32 / 20.0, &params.dimming);
        assert!(value >= previous - 1e-6, "затемнение должно расти с яркостью фона");
        previous = value;
    }
}

#[test]
fn vibrancy_changes_chroma_and_keeps_alpha() {
    let color = DisplayColor::new(0.8, 0.4, 0.2, 0.7);
    let same = vibrancy(color, 1.0);
    assert!((same.r - color.r).abs() < 1e-6 && (same.g - color.g).abs() < 1e-6 && (same.b - color.b).abs() < 1e-6);
    let gray = vibrancy(color, 0.0);
    let luma = luma_rec709(color);
    assert!((gray.r - luma).abs() < 1e-6 && (gray.g - luma).abs() < 1e-6 && (gray.b - luma).abs() < 1e-6);
    let vivid = vibrancy(color, 1.5);
    assert!(
        vivid.r - vivid.b > color.r - color.b,
        "насыщенность 1,5 должна усилить разницу каналов"
    );
    assert_eq!(vivid.a, 0.7);
    assert!(
        vibrancy(DisplayColor::rgb(1.0, 0.0, 0.0), 4.0).r <= 1.0,
        "результат зажат в 0..1"
    );
}

#[test]
fn specular_follows_the_light_axis_and_only_lives_on_the_rim() {
    let params = button_params();
    let light = LightDirection::from_angle_degrees(0.0); // свет вдоль +x
    let across = specular_intensity(PxVector::new(1.0, 0.0), light, 0.0, &params);
    let along = specular_intensity(PxVector::new(0.0, 1.0), light, 0.0, &params);
    assert!(
        (across - params.specular.alpha).abs() < 1e-6,
        "нормаль вдоль света: полный блик {across}"
    );
    assert!(along.abs() < 1e-6, "нормаль поперёк света: блика нет {along}");
    // Знак направления света не важен (модуль скалярного произведения).
    let flipped = specular_intensity(
        PxVector::new(1.0, 0.0),
        LightDirection::from_angle_degrees(180.0),
        0.0,
        &params,
    );
    assert!((flipped - across).abs() < 1e-6);
    // Глубже ободка блика нет, посередине ободка меньше, чем на краю.
    let rim = params.specular.rim_width_px;
    assert_eq!(
        specular_intensity(PxVector::new(1.0, 0.0), light, rim * 1.01, &params),
        0.0
    );
    let mid = specular_intensity(PxVector::new(1.0, 0.0), light, rim * 0.5, &params);
    assert!(mid > 0.0 && mid < across);
}

#[test]
fn dark_edge_and_press_glow_have_expected_extent() {
    let params = button_params();
    assert!((dark_edge(0.0, &params) - params.dark_edge.strength).abs() < 1e-6);
    assert_eq!(dark_edge(params.dark_edge.width_px + 1.0, &params), 0.0);
    let touch = PxPoint::new(100.0, 100.0);
    let at_touch = press_glow(touch, touch, 1.0, &params);
    assert!((at_touch - params.press.glow_alpha).abs() < 1e-6);
    let beyond = PxPoint::new(100.0 + params.press.glow_radius_px + 1.0, 100.0);
    assert_eq!(press_glow(beyond, touch, 1.0, &params), 0.0, "за радиусом свечения нет");
    assert_eq!(press_glow(touch, touch, 0.0, &params), 0.0, "без нажатия нет");
    let half = press_glow(touch, touch, 0.5, &params);
    assert!(
        (half - 0.5 * params.press.glow_alpha).abs() < 1e-6,
        "свечение растёт с силой нажатия"
    );
}

/// Стиль «чистой линзы»: все слои, кроме преломления, выключены, чтобы цвет пикселя был ровно
/// выбранным из фона.
fn lens_only_params() -> GlassParamsPx {
    let mut style = GlassStyle::default();
    style.saturation = 1.0;
    style.tint_level = 0.0;
    style.dimming.max_alpha = 0.0;
    style.dark_edge.strength = 0.0;
    style.specular.alpha = 0.0;
    style.resolve(PxSize::new(BUTTON_SIDE * 2.0, BUTTON_SIDE), PX_PER_PT)
}

/// Фон, в котором цвет кодирует координату x выборки: проверяем, откуда пиксель взял цвет.
fn x_encoded_backdrop(point: PxPoint) -> DisplayColor {
    let value = point.x / 1000.0;
    DisplayColor::rgb(value, value, value)
}

fn edge_input<'a>(params: &'a GlassParamsPx, effect: f32, opacity: Opacity) -> GlassPixelInput<'a> {
    GlassPixelInput {
        screen_pos: PxPoint::new(500.0, 300.0),
        group_sd_px: -1.0e-5, // у самого края, но уже внутри: покрытие 1 при узком сглаживании
        normal_out: PxVector::new(1.0, 0.0),
        params,
        light: LightDirection::from_angle_degrees(45.0),
        backdrop_mean_luma: 0.5,
        press: PressInput::NONE,
        effect,
        opacity,
        aa_width_px: 1.0e-5,
    }
}

#[test]
fn pixel_channels_are_sampled_outward_with_dispersion() {
    let params = lens_only_params();
    let pixel = shade_glass_pixel(x_encoded_backdrop, &edge_input(&params, 1.0, Opacity::OPAQUE));
    let strength = params.refraction_strength_px;
    let expected = |scale: f32| (500.0 + strength * scale) / 1000.0;
    assert!(
        (pixel.r - expected(1.0 - params.dispersion)).abs() < 2e-3,
        "R {}",
        pixel.r
    );
    assert!((pixel.g - expected(1.0)).abs() < 5e-3, "G {}", pixel.g);
    assert!(
        (pixel.b - expected(1.0 + params.dispersion)).abs() < 2e-3,
        "B {}",
        pixel.b
    );
    assert!(pixel.r < pixel.g && pixel.g < pixel.b, "дисперсия разводит каналы");
    assert!((pixel.a - 1.0).abs() < 2e-2, "внутри формы покрытие ≈ 1: {}", pixel.a);
}

#[test]
fn materialization_zero_leaves_backdrop_alone_and_one_is_full_effect() {
    let params = lens_only_params();
    let none = shade_glass_pixel(x_encoded_backdrop, &edge_input(&params, 0.0, Opacity::OPAQUE));
    assert_eq!(
        (none.r, none.g, none.b, none.a),
        (0.0, 0.0, 0.0, 0.0),
        "effect 0: слой не рисуется"
    );
    let half = shade_glass_pixel(x_encoded_backdrop, &edge_input(&params, 0.5, Opacity::OPAQUE));
    let full = shade_glass_pixel(x_encoded_backdrop, &edge_input(&params, 1.0, Opacity::OPAQUE));
    // Смещение растёт вместе с effect: цвет — координата выборки.
    let shift = |pixel: ui_draw::color::PremultipliedColor| pixel.g * 1000.0 - 500.0;
    assert!(
        (shift(half) - 0.5 * shift(full)).abs() < 0.5,
        "половина эффекта — половина смещения"
    );
    assert!(shift(full) > 40.0);
    // Материализация — НЕ прозрачность: при effect 0,5 слой уже непрозрачен.
    assert!((half.a - 1.0).abs() < 2e-2, "{}", half.a);
}

#[test]
fn opacity_scales_the_layer_alpha_and_outside_the_shape_is_transparent() {
    let params = lens_only_params();
    let half = Opacity::new(0.5).unwrap();
    let pixel = shade_glass_pixel(x_encoded_backdrop, &edge_input(&params, 1.0, half));
    assert!((pixel.a - 0.5).abs() < 2e-2, "{}", pixel.a);
    // Premultiplied: цвет умножен на альфу.
    assert!(pixel.g < pixel.a, "premultiplied цвет не больше альфы");
    let mut outside = edge_input(&params, 1.0, Opacity::OPAQUE);
    outside.group_sd_px = 5.0;
    let transparent = shade_glass_pixel(x_encoded_backdrop, &outside);
    assert_eq!(transparent.a, 0.0);
}

#[test]
fn bright_backdrop_gets_a_darker_tone_than_dark_backdrop() {
    let mut style = GlassStyle::default();
    style.dispersion = 0.0;
    style.saturation = 1.0;
    let params = style.resolve(PxSize::new(BUTTON_SIDE * 2.0, BUTTON_SIDE), PX_PER_PT);
    let gray = |_: PxPoint| DisplayColor::rgb(0.6, 0.6, 0.6);
    let deep_inside = |luma: f32| GlassPixelInput {
        group_sd_px: -50.0, // глубже зоны линзы, у ободка и тёмного края нет
        backdrop_mean_luma: luma,
        ..edge_input(&params, 1.0, Opacity::OPAQUE)
    };
    let over_bright = shade_glass_pixel(gray, &deep_inside(0.95));
    let over_dark = shade_glass_pixel(gray, &deep_inside(0.1));
    assert!(over_bright.g < over_dark.g, "{} против {}", over_bright.g, over_dark.g);
    // Над ярким фоном потеряно не больше потолка 35 % плюс тон.
    assert!(over_bright.g > 0.6 * (1.0 - 0.35) - params.tint_level * params.tint_alpha_at_full);
}
