//! Паритет `shade_glass_pixel`: GPU против эталона `ui_draw::glass::reference` на группе из слитых форм с нажатием
//! и светом, плюс по одному свойству материала за раз (каждое должно и совпасть с эталоном, и заметно менять картинку).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::{glass_pixel, over};
use support::{
    CLEAR, Rig, assert_no_validation_errors, empty_list, full_screen_placement, glass_group, gpu_context_or_skip,
    grid_color, lens_only_style, mean_luma_under, pixel,
};
use ui_draw::draw_list::{DrawItem, GlassGroupItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::{EdgeProfile, GlassStyle};
use ui_draw::light::LightDirection;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::transform::Affine2;
use ui_render_vulkan::BackdropInput;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};

const WIDTH: u32 = 360;
const HEIGHT: u32 = 240;
/// Допуск паритета, на канал: 0,01. Источники расхождения: fp16-цель (до 0,0005), fp16-подложка и 8-битные веса
/// билинейного фильтра (доли пикселя на пологой сетке — меньше 0,002), f32-арифметика pow/smoothstep GPU против CPU
/// (порядка 1e-5), средняя яркость из 5 выборок пирамиды (на линейной сетке почти точна). Больше 0,01 — другая формула.
const PARITY_TOLERANCE: f32 = 0.01;
/// Свойство считается «видимым», если картинка отличается от базовой линзы хотя бы на столько (на линейной сетке смещение выборки даёт цвет лишь доли процента на px, поэтому порог мал).
const VISIBLE_EFFECT: f32 = 0.01;

fn light() -> LightDirection {
    LightDirection::from_angle_degrees(30.0)
}

/// Две слитые формы: прямоугольник и круг рядом, правая нажата с касанием в стороне от центра.
fn merged_pair(style: &GlassStyle, press_amount: f32) -> GlassGroupItem {
    let rect = ShapeGeometry::RoundedRect {
        size: PxSize::new(120.0, 80.0),
        corner: CornerSpec::continuous(32.0),
    };
    let circle = ShapeGeometry::Circle { diameter: 80.0 };
    let mut group = glass_group(
        style,
        &[(rect, PxPoint::new(130.0, 120.0)), (circle, PxPoint::new(235.0, 120.0))],
        24.0,
    );
    let scale = group.params.press.scale_for(press_amount);
    group.members[1].transform = Affine2::uniform_scale(scale).then(Affine2::translate(235.0, 120.0));
    group.members[1].press.amount = press_amount;
    group.members[1].press.touch_point_local = PxPoint::new(-15.0, 10.0);
    group
}

struct Comparison {
    worst: f32,
    worst_at: (u32, u32),
    max_effect_vs_clear: f32,
    pixels: Vec<[f32; 4]>,
}

fn compare(
    context: &gpu_vulkan::context::GpuContext,
    group: &GlassGroupItem,
    effect_baseline: Option<&[[f32; 4]]>,
) -> Comparison {
    let backdrop = SyntheticBackdrop::new(
        context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::CoordinateGrid,
    )
    .unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let mut list = empty_list();
    list.light = light();
    list.items.push(DrawItem::Glass(group.clone()));
    let input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    let (_, pixels) = rig.render(&list, input);

    let backdrop_color = |point: PxPoint| grid_color(point, WIDTH as f32, HEIGHT as f32);
    let mean_luma = mean_luma_under(group, backdrop_color);
    let bounds = group.bounds_px().unwrap();
    let (mut worst, mut worst_at, mut max_effect) = (0.0_f32, (0, 0), 0.0_f32);
    for y in (bounds.origin.y.max(0.0) as u32..(bounds.max_y() as u32).min(HEIGHT)).step_by(3) {
        for x in (bounds.origin.x.max(0.0) as u32..(bounds.max_x() as u32).min(WIDTH)).step_by(3) {
            let point = PxPoint::new(x as f32 + 0.5, y as f32 + 0.5);
            let expected = over(CLEAR, glass_pixel(group, light(), point, mean_luma, backdrop_color));
            let actual = pixel(&pixels, WIDTH, x, y);
            for channel in 0..3 {
                let difference = (actual[channel] - expected[channel]).abs();
                if difference > worst {
                    worst = difference;
                    worst_at = (x, y);
                }
            }
            if let Some(baseline) = effect_baseline {
                let base = pixel(baseline, WIDTH, x, y);
                max_effect = max_effect.max((0..3).map(|c| (actual[c] - base[c]).abs()).fold(0.0, f32::max));
            }
        }
    }
    Comparison {
        worst,
        worst_at,
        max_effect_vs_clear: max_effect,
        pixels,
    }
}

#[test]
fn merged_pressed_group_with_light_matches_shade_glass_pixel() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let style = GlassStyle::default();
    // Тень стекла остаётся включённой: она тоже часть эталона (`shade_glass_with_shadow`).
    let mut group = merged_pair(&style, 0.8);
    group.effect = 0.85;
    group.opacity = ui_draw::color::Opacity::new(0.9).unwrap();
    let result = compare(&context, &group, None);
    println!(
        "паритет группы: наибольшее расхождение {} в {:?}",
        result.worst, result.worst_at
    );
    assert!(
        result.worst <= PARITY_TOLERANCE,
        "расхождение {} в пикселе {:?}",
        result.worst,
        result.worst_at
    );
    // Страховка: стекло реально нарисовано (центр формы отличается от цвета очистки).
    let centre = pixel(&result.pixels, WIDTH, 130, 120);
    assert!((centre[0] - CLEAR[0]).abs() > 0.05, "стекло не нарисовано: {centre:?}");
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn each_material_property_matches_the_reference_and_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base_style = lens_only_style();
    let lens = compare(&context, &merged_pair(&base_style, 0.0), None);
    assert!(
        lens.worst <= PARITY_TOLERANCE,
        "линза: {} в {:?}",
        lens.worst,
        lens.worst_at
    );

    type Tweak = fn(&mut GlassStyle);
    let cases: [(&str, Tweak, f32); 9] = [
        ("насыщенность", |s| s.saturation = 1.9, 0.0),
        (
            "тон (плёнка)",
            |s| {
                s.tint_level = 1.0;
                s.tint_alpha_at_full = 0.7;
            },
            0.0,
        ),
        ("блик", |s| s.specular.alpha = 0.9, 0.0),
        ("тёмный край", |s| s.dark_edge.strength = 0.8, 0.0),
        (
            "затемнение",
            |s| {
                s.dimming.max_alpha = 0.6;
                s.dimming.luminance_low = 0.0;
                s.dimming.luminance_high = 0.6;
            },
            0.0,
        ),
        ("свечение нажатия", |s| s.press.glow_alpha = 0.7, 1.0),
        ("усиление преломления нажатием", |s| s.press.refraction_boost = 0.8, 1.0), /* не больше: выборка не должна выйти за картинку фона */
        (
            "профиль «окружность»",
            |s| s.edge_profile = EdgeProfile::QuarterCircle,
            0.0,
        ),
        (
            "зона линзы шире",
            |s| s.lens_zone = ui_draw::glass::Extent::FractionOfHeight(0.45),
            0.0,
        ),
    ];
    for (name, tweak, press) in cases {
        let mut style = base_style;
        tweak(&mut style);
        let reference_lens = if press > 0.0 {
            compare(&context, &merged_pair(&base_style, press), None).pixels
        } else {
            lens.pixels.clone()
        };
        let result = compare(&context, &merged_pair(&style, press), Some(&reference_lens));
        println!(
            "{name}: расхождение {} в {:?}, эффект {}",
            result.worst, result.worst_at, result.max_effect_vs_clear
        );
        assert!(
            result.worst <= PARITY_TOLERANCE,
            "{name}: расхождение с эталоном {} в {:?}",
            result.worst,
            result.worst_at
        );
        assert!(
            result.max_effect_vs_clear >= VISIBLE_EFFECT,
            "{name}: свойство не меняет картинку (эффект {})",
            result.max_effect_vs_clear
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn materialization_zero_leaves_the_backdrop_untouched_and_one_is_full_glass() {
    let Some(context) = gpu_context_or_skip() else { return };
    let style = GlassStyle {
        shadow: ui_draw::glass::ShadowStyle {
            alpha: 0.0,
            ..GlassStyle::default().shadow
        },
        ..GlassStyle::default()
    };
    let mut group = merged_pair(&style, 0.0);
    group.effect = 0.0;
    let hidden = compare(&context, &group, None);
    let centre = pixel(&hidden.pixels, WIDTH, 130, 120);
    for channel in 0..3 {
        assert!(
            (centre[channel] - CLEAR[channel]).abs() < 1e-3,
            "effect = 0: цель не должна меняться, {centre:?}"
        );
    }
    assert!(hidden.worst <= PARITY_TOLERANCE);
    assert_no_validation_errors(&context);
}
