//! Пирамида размытия фона: перестройка только при смене ключа, путь Recompose, фон Absent, размещение фона,
//! реальное размытие, пересоздание под новый размер и тень стекла.

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
use support::{
    CLEAR, Rig, assert_no_validation_errors, center_of, empty_list, full_screen_placement, glass_group,
    gpu_context_or_skip, lens_only_style, pixel,
};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxRect, PxSize};
use ui_draw::glass::{Extent, GlassStyle};
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::{BackdropInput, BackdropPlacement, PyramidOutcome};

const WIDTH: u32 = 360;
const HEIGHT: u32 = 240;

fn extent() -> vk::Extent2D {
    vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    }
}

fn card(center: PxPoint, size: PxSize) -> ShapeGeometry {
    let _ = center;
    ShapeGeometry::RoundedRect {
        size,
        corner: CornerSpec::continuous(24.0),
    }
}

fn single_glass_list(style: &GlassStyle, center: PxPoint, size: PxSize) -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    list.items.push(DrawItem::Glass(glass_group(
        style,
        &[(card(center, size), center)],
        0.0,
    )));
    list
}

#[test]
fn same_backdrop_frame_reuses_the_pyramid_and_a_new_frame_rebuilds_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let red = SyntheticBackdrop::new(&context, extent(), BackdropPattern::Uniform([0.8, 0.1, 0.1])).unwrap();
    let blue = SyntheticBackdrop::new(&context, extent(), BackdropPattern::Uniform([0.1, 0.1, 0.8])).unwrap();
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let list = single_glass_list(
        &lens_only_style(),
        PxPoint::new(180.0, 120.0),
        PxSize::new(160.0, 100.0),
    );
    let placement = full_screen_placement(WIDTH as f32, HEIGHT as f32);

    let frame_1 = red.source(1);
    let (first, first_pixels) = rig.render(
        &list,
        BackdropInput::Live {
            source: &frame_1,
            placement,
        },
    );
    let (second, second_pixels) = rig.render(
        &list,
        BackdropInput::Live {
            source: &frame_1,
            placement,
        },
    );
    assert_eq!(
        (first.pyramid, second.pyramid),
        (PyramidOutcome::Rebuilt, PyramidOutcome::Reused)
    );
    assert_eq!(
        first_pixels, second_pixels,
        "переиспользованная пирамида даёт тот же кадр"
    );
    let counters = rig.renderer.counters();
    assert_eq!(
        (
            counters.pyramid_rebuilds,
            counters.pyramid_reuses,
            counters.frames_recorded
        ),
        (1, 1, 2)
    );

    // Новый номер кадра той же картинки — ключ сменился, ступени записаны заново.
    let frame_2 = red.source(2);
    let (third, _) = rig.render(
        &list,
        BackdropInput::Live {
            source: &frame_2,
            placement,
        },
    );
    assert_eq!(third.pyramid, PyramidOutcome::Rebuilt);
    // Другая картинка (другое поколение) — тоже перестройка, и цвет стекла следует за новым фоном.
    let blue_source = blue.source(2);
    let (fourth, blue_pixels) = rig.render(
        &list,
        BackdropInput::Live {
            source: &blue_source,
            placement,
        },
    );
    assert_eq!(fourth.pyramid, PyramidOutcome::Rebuilt);
    let centre = pixel(&blue_pixels, WIDTH, 180, 120);
    assert!(
        centre[2] > 0.5 && centre[0] < 0.3,
        "стекло показывает синий фон: {centre:?}"
    );
    let red_centre = pixel(&first_pixels, WIDTH, 180, 120);
    assert!(
        red_centre[0] > 0.5 && red_centre[2] < 0.3,
        "и красный в первом кадре: {red_centre:?}"
    );
    // Другое размещение фона при том же кадре — тоже перестройка (оно запечено в ступенях).
    let moved = BackdropPlacement {
        destination: PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(300.0, 240.0)),
    };
    let (fifth, _) = rig.render(
        &list,
        BackdropInput::Live {
            source: &blue_source,
            placement: moved,
        },
    );
    assert_eq!(fifth.pyramid, PyramidOutcome::Rebuilt);
    // Кадр без стекла пирамиду не трогает вовсе.
    let (plain, _) = rig.render(
        &empty_list(),
        BackdropInput::Live {
            source: &blue_source,
            placement: moved,
        },
    );
    assert_eq!(plain.pyramid, PyramidOutcome::NotNeeded);
    assert_eq!(rig.renderer.counters().pyramid_rebuilds, 4);
    drop((rig, red, blue, frame_1, frame_2, blue_source));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn absent_backdrop_puts_glass_over_the_fill_color_and_outside_placement_is_black() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let list = single_glass_list(
        &lens_only_style(),
        PxPoint::new(180.0, 120.0),
        PxSize::new(160.0, 100.0),
    );
    let fill = DisplayColor::rgb(0.6, 0.4, 0.2);
    let (report, pixels) = rig.render(&list, BackdropInput::Absent { fill });
    assert_eq!(report.pyramid, PyramidOutcome::Rebuilt);
    // Стекло с линзой над сплошной заливкой: везде внутри формы цвет заливки (и в зоне линзы тоже).
    for (x, y) in [(180, 120), (102, 120), (180, 72), (257, 120)] {
        let value = pixel(&pixels, WIDTH, x, y);
        for (channel, want) in [fill.r, fill.g, fill.b].into_iter().enumerate() {
            assert!(
                (value[channel] - want).abs() < 2e-3,
                "({x},{y}) канал {channel}: {value:?}, заливка {fill:?}"
            );
        }
    }
    // Повтор с той же заливкой — переиспользование; другая заливка — перестройка и новый цвет.
    let (again, _) = rig.render(&list, BackdropInput::Absent { fill });
    assert_eq!(again.pyramid, PyramidOutcome::Reused);
    let other = DisplayColor::rgb(0.1, 0.5, 0.9);
    let (changed, other_pixels) = rig.render(&list, BackdropInput::Absent { fill: other });
    assert_eq!(changed.pyramid, PyramidOutcome::Rebuilt);
    assert!((pixel(&other_pixels, WIDTH, 180, 120)[2] - other.b).abs() < 2e-3);

    // Живой фон занимает только левую половину экрана: стекло над правой половиной видит чёрное.
    let backdrop = SyntheticBackdrop::new(&context, extent(), BackdropPattern::Uniform([0.9, 0.9, 0.9])).unwrap();
    let source = backdrop.source(1);
    let left_half = BackdropPlacement {
        destination: PxRect::new(PxPoint::new(0.0, 0.0), PxSize::new(180.0, 240.0)),
    };
    let wide = single_glass_list(
        &lens_only_style(),
        PxPoint::new(180.0, 120.0),
        PxSize::new(300.0, 100.0),
    );
    let (_, pixels) = rig.render(
        &wide,
        BackdropInput::Live {
            source: &source,
            placement: left_half,
        },
    );
    let left = pixel(&pixels, WIDTH, 100, 120);
    let right = pixel(&pixels, WIDTH, 260, 120);
    assert!(left[0] > 0.85, "слева виден фон: {left:?}");
    assert!(
        right[0] < 0.02 && right[1] < 0.02,
        "справа за границей размещения фон чёрный: {right:?}"
    );
    assert_no_validation_errors(&context);
}

/// Стандартное отклонение яркости в прямоугольнике пикселей.
fn luma_deviation(
    pixels: &[[f32; 4]],
    width: u32,
    x_range: std::ops::Range<u32>,
    y_range: std::ops::Range<u32>,
) -> f32 {
    let values: Vec<f32> = y_range
        .flat_map(|y| x_range.clone().map(move |x| (x, y)))
        .map(|(x, y)| {
            let p = pixel(pixels, width, x, y);
            0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
        })
        .collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt()
}

#[test]
fn blur_radius_actually_blurs_a_checkerboard() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = SyntheticBackdrop::new(&context, extent(), BackdropPattern::Photo).unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    // Стекло без линзы над шахматкой (левая треть, клетки 24 px): меняется только радиус размытия.
    let render_with_blur = |rig: &mut Rig, blur_pt: f32| {
        let mut style = lens_only_style();
        style.lens_zone = Extent::Points(0.0);
        style.refraction_strength = Extent::Points(0.0);
        style.blur_radius_pt = blur_pt;
        let list = single_glass_list(&style, PxPoint::new(60.0, 140.0), PxSize::new(80.0, 80.0));
        rig.render(&list, input).1
    };
    let sharp = render_with_blur(&mut rig, 0.0);
    let blurred = render_with_blur(&mut rig, 8.0);
    let sharp_deviation = luma_deviation(&sharp, WIDTH, 30..90, 110..170);
    let blurred_deviation = luma_deviation(&blurred, WIDTH, 30..90, 110..170);
    println!("контраст шахматки: резко {sharp_deviation}, размыто {blurred_deviation}");
    assert!(sharp_deviation > 0.25, "шахматка видна без размытия: {sharp_deviation}");
    assert!(
        blurred_deviation < 0.5 * sharp_deviation,
        "размытие 24 px обязано гасить клетки 24 px: {blurred_deviation} против {sharp_deviation}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn changing_logical_size_recreates_the_pyramid_and_keeps_rendering_correct() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let backdrop = SyntheticBackdrop::new(&context, extent(), BackdropPattern::Uniform([0.7, 0.3, 0.2])).unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let list = single_glass_list(&lens_only_style(), PxPoint::new(100.0, 80.0), PxSize::new(100.0, 60.0));
    let first_input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    rig.render(&list, first_input);
    rig.resize((200, 160));
    let second_input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(200.0, 160.0),
    };
    let (report, pixels) = rig.render(&list, second_input);
    assert_eq!(report.pyramid, PyramidOutcome::Rebuilt);
    assert_eq!(rig.renderer.counters().pyramid_recreations, 1);
    let centre = pixel(&pixels, 200, 100, 80);
    assert!(
        (centre[0] - 0.7).abs() < 3e-3 && (centre[2] - 0.2).abs() < 3e-3,
        "{centre:?}"
    );
    drop((rig, source, backdrop));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn glass_shadow_darkens_below_the_shape_and_leaves_the_top_clean() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let mut style = lens_only_style();
    style.shadow.alpha = 0.6;
    style.shadow.radius_pt = 6.0; // 18 px
    let list = single_glass_list(&style, PxPoint::new(180.0, 100.0), PxSize::new(120.0, 60.0));
    let (_, pixels) = rig.render(
        &list,
        BackdropInput::Absent {
            fill: DisplayColor::WHITE,
        },
    );
    // Форма: y от 70 до 130. Смещение тени вниз — радиус × offset_ratio; ниже края темнее, выше края фон как был.
    let below = pixel(&pixels, WIDTH, 180, 136);
    let above = pixel(&pixels, WIDTH, 180, 50);
    assert!((above[0] - CLEAR[0]).abs() < 1e-3, "над формой тени нет: {above:?}");
    assert!(
        CLEAR[0] - below[0] > 0.02,
        "под формой лежит тень: {below:?} против {CLEAR:?}"
    );
    let _ = center_of(0, 0);
    assert_no_validation_errors(&context);
}
