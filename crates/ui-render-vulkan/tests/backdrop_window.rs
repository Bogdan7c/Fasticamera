//! Окно чтения фона (сессия 13): подложка, у которой кадр лежит в левом верхнем прямоугольнике картинки
//! постоянной ёмкости, показывается зеркально или увеличенной. Стекло обязано видеть то же, что пользователь:
//! только содержимое кадра (не остатки прежних кадров за его краем), в том же зеркале и с тем же увеличением.
//!
//! Фон — «координатная сетка» (цвет = координата тексела / размер картинки), поэтому прочитанный цвет в
//! глубине формы стекла (там нет преломления) говорит, из какого тексела фона взят пиксель.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::{
    Rig, assert_no_validation_errors, empty_list, full_screen_placement, glass_group, gpu_context_or_skip,
    lens_only_style, pixel,
};
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};
use ui_render_vulkan::{BackdropInput, PyramidOutcome};

const WIDTH: u32 = 360;
const HEIGHT: u32 = 240;
/// Допуск по координате тексела, px: билинейная выборка и квантование цвета.
const TEXEL_TOLERANCE: f32 = 1.5;

fn list_with_big_glass(style: &GlassStyle) -> ui_draw::draw_list::DrawList {
    let size = PxSize::new(320.0, 200.0);
    let mut list = empty_list();
    list.items.push(DrawItem::Glass(glass_group(
        style,
        &[(
            ShapeGeometry::RoundedRect {
                size,
                corner: CornerSpec::continuous(24.0),
            },
            PxPoint::new(180.0, 120.0),
        )],
        0.0,
    )));
    list
}

/// Тексел фона по цвету пикселя сетки: R·W.
fn texel_x(pixel: [f32; 4]) -> f32 {
    pixel[0] * WIDTH as f32
}

fn render_window(
    context: &gpu_vulkan::context::GpuContext,
    style: &GlassStyle,
    window: SourceWindow,
) -> (PyramidOutcome, Vec<[f32; 4]>) {
    let backdrop = SyntheticBackdrop::new(
        context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::CoordinateGrid,
    )
    .unwrap();
    let source = backdrop.source(1).with_window(window).unwrap();
    let mut rig = Rig::new(
        context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let (report, pixels) = rig.render(
        &list_with_big_glass(style),
        BackdropInput::Live {
            source: &source,
            placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
        },
    );
    (report.pyramid, pixels)
}

fn content_left_half() -> SourceWindow {
    SourceWindow::of_content(vk::Extent2D {
        width: WIDTH / 2,
        height: HEIGHT,
    })
}

/// Резкий путь (шейдер стекла читает фон сам): содержимое — левая половина картинки, экран целиком показывает
/// именно её; зеркало разворачивает её по горизонтали; увеличение ×2 берёт центральную половину содержимого.
#[test]
fn glass_sharp_sampling_reads_only_the_content_window_mirrored_and_magnified() {
    let Some(context) = gpu_context_or_skip() else { return };
    let style = lens_only_style();
    // Пиксель (130, 120) глубоко в форме (она шире 300 px): преломления нет. Относительная x = 130,5/360.
    let probe = |window: SourceWindow| {
        let (_, pixels) = render_window(&context, &style, window);
        texel_x(pixel(&pixels, WIDTH, 130, 120))
    };
    let relative = 130.5 / WIDTH as f32;
    let plain = probe(content_left_half());
    assert!(
        (plain - relative * 180.0).abs() < TEXEL_TOLERANCE,
        "содержимое 180 px растянуто на экран: тексел {plain} вместо {}",
        relative * 180.0
    );
    let mirrored = probe(content_left_half().with_mirror(HorizontalMirror::On));
    assert!(
        (mirrored - (180.0 - relative * 180.0)).abs() < TEXEL_TOLERANCE,
        "зеркало: тексел {mirrored} вместо {}",
        180.0 - relative * 180.0
    );
    let magnified = probe(content_left_half().with_magnification(2.0).unwrap());
    let expected = 45.0 + relative * 90.0;
    assert!(
        (magnified - expected).abs() < TEXEL_TOLERANCE,
        "увеличение ×2: видна центральная половина (тексел 45…135): {magnified} вместо {expected}"
    );
    assert_no_validation_errors(&context);
}

/// Путь пирамиды (размытый фон: первая ступень читает фон через окно) и ключ пирамиды: смена окна при том же
/// кадре подложки перестраивает ступени, а то же окно — переиспользует.
#[test]
fn blurred_glass_follows_the_window_and_a_changed_window_rebuilds_the_pyramid() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut style = lens_only_style();
    style.blur_radius_pt = 8.0;
    let backdrop = SyntheticBackdrop::new(
        &context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::CoordinateGrid,
    )
    .unwrap();
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let list = list_with_big_glass(&style);
    let placement = full_screen_placement(WIDTH as f32, HEIGHT as f32);
    let mut render = |window: SourceWindow| {
        let source = backdrop.source(1).with_window(window).unwrap();
        rig.render(
            &list,
            BackdropInput::Live {
                source: &source,
                placement,
            },
        )
    };
    let (first, plain_pixels) = render(content_left_half());
    assert_eq!(first.pyramid, PyramidOutcome::Rebuilt);
    let (second, _) = render(content_left_half());
    assert_eq!(
        second.pyramid,
        PyramidOutcome::Reused,
        "то же окно, тот же кадр — ступени переиспользованы"
    );
    let (third, mirrored_pixels) = render(content_left_half().with_mirror(HorizontalMirror::On));
    assert_eq!(
        third.pyramid,
        PyramidOutcome::Rebuilt,
        "окно сменилось — размытие устарело"
    );
    let (fourth, _) = render(content_left_half().with_magnification(1.5).unwrap());
    assert_eq!(fourth.pyramid, PyramidOutcome::Rebuilt);

    // Размытая линейная сетка остаётся линейной: тексел в точке (130, 120) — как у резкого пути (допуск шире).
    let relative = 130.5 / WIDTH as f32;
    let (plain, mirrored) = (
        texel_x(pixel(&plain_pixels, WIDTH, 130, 120)),
        texel_x(pixel(&mirrored_pixels, WIDTH, 130, 120)),
    );
    assert!(
        (plain - relative * 180.0).abs() < 4.0,
        "размытие: {plain} вместо {}",
        relative * 180.0
    );
    assert!(
        (mirrored - (180.0 - relative * 180.0)).abs() < 4.0,
        "размытие в зеркале: {mirrored} вместо {}",
        180.0 - relative * 180.0
    );
    assert!(
        mirrored > plain + 20.0,
        "зеркало действительно развернуло фон под размытием"
    );
    drop(rig);
    assert_no_validation_errors(&context);
}
