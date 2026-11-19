//! Свойства стекла числами: линза смещает точку выборки фона, дисперсия разводит каналы, тон зависит от яркости
//! фона. Фон — «сетка координат» (цвет пикселя = его координата), поэтому по readback читается, ОТКУДА стекло взяло цвет.

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
    Rig, assert_no_validation_errors, center_of, empty_list, full_screen_placement, glass_group, gpu_context_or_skip,
    lens_only_style, pixel,
};
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::GlassStyle;
use ui_draw::glass::reference::{backdrop_dimming, lens_profile, refraction_offsets};
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_render_vulkan::BackdropInput;
use ui_render_vulkan::testing::{BackdropPattern, SyntheticBackdrop};

const WIDTH: u32 = 360;
const HEIGHT: u32 = 240;
/// Допуск координаты выборки, px: цель fp16 (шаг у значений около 1 — 0,0005 → 0,18 px на ширине 360), плюс
/// fp16-хранение самой подложки и 8-битные веса билинейного фильтра — вместе не больше ≈ 0,35 px. Берём 0,5 из задания.
const SAMPLE_TOLERANCE_PX: f32 = 0.5;

fn card_geometry() -> ShapeGeometry {
    ShapeGeometry::RoundedRect {
        size: PxSize::new(160.0, 100.0),
        corner: CornerSpec::continuous(40.0),
    }
}

fn lens_scene(style: &GlassStyle) -> (ui_draw::draw_list::DrawList, ui_draw::draw_list::GlassGroupItem) {
    let group = glass_group(style, &[(card_geometry(), PxPoint::new(180.0, 120.0))], 0.0);
    let mut list = empty_list();
    list.items.push(DrawItem::Glass(group.clone()));
    (list, group)
}

#[test]
fn lens_shifts_the_sample_point_only_near_the_edge_and_splits_channels() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let backdrop = SyntheticBackdrop::new(
        &context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::CoordinateGrid,
    )
    .unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let mut style = lens_only_style();
    style.dispersion = 0.2; // усиленная дисперсия, чтобы разведение каналов заметно превышало допуск
    let (list, group) = lens_scene(&style);
    let input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    let (_, pixels) = rig.render(&list, input);
    let params = group.params;

    // Центр формы: глубже зоны линзы смещения нет — выборка из своей же точки.
    let centre = pixel(&pixels, WIDTH, 180, 120);
    let (sample_x, sample_y) = (centre[0] * WIDTH as f32, centre[1] * HEIGHT as f32);
    assert!(
        (sample_x - 180.5).abs() < SAMPLE_TOLERANCE_PX && (sample_y - 120.5).abs() < SAMPLE_TOLERANCE_PX,
        "центр: выборка ({sample_x}, {sample_y}) вместо (180.5, 120.5)"
    );

    // Пиксели у левого, правого, верхнего краёв и у угла: GPU совпадает с refraction_offsets эталона.
    let probes = [(102_u32, 120_u32), (257, 120), (180, 72), (112, 80), (250, 160)];
    for (x, y) in probes {
        let point = center_of(x, y);
        let field = group.field_at(point).unwrap().unwrap();
        let offsets = refraction_offsets(field.distance_px, field.normal_out, &params, 0.0);
        let value = pixel(&pixels, WIDTH, x, y);
        // R читает X красного канала сетки, G — Y зелёного, B — X синего: смещение по каналам из эталона.
        let actual = (
            value[0] * WIDTH as f32 - point.x,
            value[1] * HEIGHT as f32 - point.y,
            value[2] * WIDTH as f32 - point.x,
        );
        assert!(
            (actual.0 - offsets.r.dx).abs() < SAMPLE_TOLERANCE_PX,
            "({x},{y}) R.dx: GPU {} против эталона {}",
            actual.0,
            offsets.r.dx
        );
        assert!(
            (actual.1 - offsets.g.dy).abs() < SAMPLE_TOLERANCE_PX,
            "({x},{y}) G.dy: GPU {} против эталона {}",
            actual.1,
            offsets.g.dy
        );
        assert!(
            (actual.2 - offsets.b.dx).abs() < SAMPLE_TOLERANCE_PX,
            "({x},{y}) B.dx: GPU {} против эталона {}",
            actual.2,
            offsets.b.dx
        );
    }

    // У левого края смещение не меньше порога, выведенного из параметров, и направлено НАРУЖУ (влево).
    let point = center_of(102, 120);
    let depth = -group.field_at(point).unwrap().unwrap().distance_px;
    let threshold = 0.5 * params.refraction_strength_px * lens_profile(depth, params.lens_zone_px, params.edge_profile);
    let value = pixel(&pixels, WIDTH, 102, 120);
    let shift = value[0] * WIDTH as f32 - point.x;
    assert!(
        -shift >= threshold && threshold > 3.0,
        "смещение у края {shift} px при пороге {threshold}"
    );

    // Дисперсия: красный и синий каналы взяты из разных точек. Разница = 2δ·смещение зелёного, здесь ≈ 4,6 px.
    let spread = (value[2] - value[0]) * WIDTH as f32;
    let offsets = refraction_offsets(
        group.field_at(point).unwrap().unwrap().distance_px,
        group.field_at(point).unwrap().unwrap().normal_out,
        &params,
        0.0,
    );
    assert!(
        spread.abs() > 2.0 && (spread - (offsets.b.dx - offsets.r.dx)).abs() < SAMPLE_TOLERANCE_PX,
        "R и B разведены дисперсией: GPU {spread} px, эталон {} px",
        offsets.b.dx - offsets.r.dx
    );
    drop((rig, source, backdrop));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn zero_dispersion_collapses_red_and_blue_to_the_same_sample() {
    let Some(context) = gpu_context_or_skip() else { return };
    let backdrop = SyntheticBackdrop::new(
        &context,
        vk::Extent2D {
            width: WIDTH,
            height: HEIGHT,
        },
        BackdropPattern::CoordinateGrid,
    )
    .unwrap();
    let source = backdrop.source(1);
    let mut rig = Rig::new(
        &context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba16Float,
    );
    let mut style = lens_only_style();
    style.dispersion = 0.0;
    let (list, _) = lens_scene(&style);
    let input = BackdropInput::Live {
        source: &source,
        placement: full_screen_placement(WIDTH as f32, HEIGHT as f32),
    };
    let (_, pixels) = rig.render(&list, input);
    let value = pixel(&pixels, WIDTH, 102, 120);
    assert!(
        ((value[2] - value[0]) * WIDTH as f32).abs() < 0.3,
        "без дисперсии R и B совпадают: {value:?}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn tone_darkens_more_over_a_bright_backdrop_than_over_a_dark_one() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut style = lens_only_style();
    style.dimming = GlassStyle::default().dimming;
    let (list, group) = lens_scene(&style);
    let centre_after = |level: f32| {
        let backdrop = SyntheticBackdrop::new(
            &context,
            vk::Extent2D {
                width: WIDTH,
                height: HEIGHT,
            },
            BackdropPattern::Uniform([level; 3]),
        )
        .unwrap();
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
        pixel(&rig.render(&list, input).1, WIDTH, 180, 120)[0]
    };
    let (white, grey) = (centre_after(1.0), centre_after(0.2));
    let dim_white = backdrop_dimming(1.0, &group.params.dimming);
    let dim_grey = backdrop_dimming(0.2, &group.params.dimming);
    assert!(
        dim_white > dim_grey + 0.05,
        "в стиле по умолчанию белый фон затемняется сильнее серого: {dim_white} против {dim_grey}"
    );
    // Результат = цвет фона × (1 − затемнение): эталонная формула, допуск fp16-цели 1/1000.
    assert!(
        (white - (1.0 - dim_white)).abs() < 1.5e-3,
        "белый фон: GPU {white}, эталон {}",
        1.0 - dim_white
    );
    assert!(
        (grey - 0.2 * (1.0 - dim_grey)).abs() < 1.5e-3,
        "серый фон: GPU {grey}, эталон {}",
        0.2 * (1.0 - dim_grey)
    );
    assert!(
        1.0 - white > (0.2 - grey) / 0.2 + 0.05,
        "затемнение белого ({}) сильнее, чем серого ({})",
        1.0 - white,
        (0.2 - grey) / 0.2
    );
    assert_no_validation_errors(&context);
}
