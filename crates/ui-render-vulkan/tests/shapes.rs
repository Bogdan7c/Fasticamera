//! Тесты фигур: паритет GPU ↔ эталон `ui_draw::shape` (покрытие × цвет, обводка, тень), непрерывное
//! скругление против дуги окружности, порядок рисования и прозрачность.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::{render_shapes, worst_difference};
use support::{
    CLEAR, Rig, assert_no_validation_errors, center_of, empty_list, gpu_context_or_skip, pixel, shape, shape_item,
};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::StrokeStyle;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::glass::ShadowParamsPx;
use ui_draw::shape::{CornerSpec, ShapeGeometry, corrected_distance, coverage};
use ui_draw::transform::Affine2;
use ui_render_vulkan::{BackdropInput, PyramidOutcome};

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};

/// Допуск паритета фигур: 2/255 на канал. Цель Rgba8: округление записи ±0,5/255, смешивание в 8 битах ещё
/// ±0,5/255, разница f32-арифметики GPU и CPU в smoothstep и pow — на порядок меньше. Больше 2/255 — уже
/// другая формула, а не шум.
const SHAPE_TOLERANCE: f32 = 2.0 / 255.0;

fn mixed_shapes() -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    let mut rect = shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(80.0, 60.0),
            corner: CornerSpec::continuous(20.0),
        },
        Affine2::translate(50.0, 50.0),
        DisplayColor::rgb(0.9, 0.2, 0.2),
    );
    rect.stroke = Some(StrokeStyle {
        width_px: 4.0,
        color: DisplayColor::WHITE,
    });
    list.items.push(shape(rect));
    let mut capsule = shape_item(
        ShapeGeometry::Capsule {
            size: PxSize::new(90.0, 40.0),
        },
        Affine2::translate(150.0, 40.0),
        DisplayColor::rgb(0.2, 0.7, 0.3),
    );
    capsule.opacity = Opacity::new(0.8).unwrap();
    list.items.push(shape(capsule));
    let mut circle = shape_item(
        ShapeGeometry::Circle { diameter: 50.0 },
        Affine2::translate(50.0, 105.0),
        DisplayColor::new(0.2, 0.3, 0.9, 0.7),
    );
    circle.shadow = Some(ShadowParamsPx {
        radius_px: 8.0,
        alpha: 0.5,
        offset_px: 4.0,
    });
    list.items.push(shape(circle));
    // Повёрнутая и растянутая фигура с дугой окружности: проверяет путь обратной матрицы и масштаб градиента.
    let skewed = shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(60.0, 40.0),
            corner: CornerSpec::circular(14.0),
        },
        Affine2::rotate(0.35)
            .then(Affine2::scale(1.0, 1.4))
            .then(Affine2::translate(170.0, 100.0)),
        DisplayColor::rgb(0.8, 0.6, 0.1),
    );
    list.items.push(shape(skewed));
    list
}

#[test]
fn gpu_shapes_match_the_cpu_reference_pixel_by_pixel() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let mut rig = Rig::new(
        &context,
        (240, 140),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let list = mixed_shapes();
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(
        (report.shape_instances, report.glass_groups, report.pyramid),
        (4, 0, PyramidOutcome::NotNeeded)
    );
    let expected = render_shapes(&list, 240, 140, CLEAR);
    let (worst, at) = worst_difference(&pixels, &expected, 240);
    assert!(
        worst <= SHAPE_TOLERANCE,
        "наибольшее расхождение {worst} в пикселе {at:?}, допуск {SHAPE_TOLERANCE}"
    );
    // Страховка от теста «сравнили пустое с пустым»: в кадре реально нарисованы фигуры.
    assert!(pixel(&pixels, 240, 50, 50)[0] > 0.8, "красный прямоугольник нарисован");
    assert!(pixel(&pixels, 240, 150, 40)[1] > 0.5, "капсула нарисована");
    drop((rig, list));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn continuous_corner_follows_the_superellipse_not_a_circle() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Квадрат 200×200, радиус угла 40: центр угловой зоны (160, 160), края суперэллипса на диагонали при t ≈ 34,8.
    let geometry = ShapeGeometry::RoundedRect {
        size: PxSize::new(120.0, 120.0),
        corner: CornerSpec::continuous(40.0),
    };
    let mut list = empty_list();
    list.items.push(shape(shape_item(
        geometry,
        Affine2::translate(100.0, 100.0),
        DisplayColor::WHITE,
    )));
    let mut rig = Rig::new(
        &context,
        (200, 200),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let (_, pixels) = rig.render(&list, ABSENT);

    // Точка на диагонали угла на расстоянии t = 32 от центра угловой зоны по каждой оси: ВНУТРИ суперэллипса
    // (|q|₅ = 37,3 < 40), но ВНЕ дуги окружности (|q|₂ = 45,3 > 40).
    let probe = center_of(100 + 60 - 40 + 32, 100 + 60 - 40 + 32);
    let corner_center = PxPoint::new(120.0, 120.0);
    let radial = ((probe.x - corner_center.x).powi(2) + (probe.y - corner_center.y).powi(2)).sqrt();
    assert!(
        radial - 40.0 > 5.0,
        "точка отстоит от дуги на {} px, нужно > 5",
        radial - 40.0
    );
    let inside = pixel(&pixels, 200, probe.x as u32, probe.y as u32);
    assert!(
        inside[0] > 0.9,
        "внутри суперэллипса пиксель закрашен, получено {inside:?}"
    );

    // И наоборот: на самой границе суперэллипса (t ≈ 34,8) покрытие около половины, как в эталоне.
    let edge = center_of(120 + 34, 120 + 34);
    let expected = coverage(
        corrected_distance(&geometry, PxPoint::new(edge.x - 100.0, edge.y - 100.0)).distance,
        1.0,
    );
    let actual_edge = pixel(&pixels, 200, 154, 154);
    let actual = (actual_edge[0] - CLEAR[0]) / (1.0 - CLEAR[0]);
    assert!(
        (actual - expected).abs() < 0.04,
        "покрытие на краю: GPU {actual}, эталон {expected}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn draw_order_is_list_order_and_item_opacity_blends() {
    let Some(context) = gpu_context_or_skip() else { return };
    let square = ShapeGeometry::RoundedRect {
        size: PxSize::new(40.0, 40.0),
        corner: CornerSpec::circular(0.0),
    };
    let mut list = empty_list();
    list.items.push(shape(shape_item(
        square,
        Affine2::translate(30.0, 30.0),
        DisplayColor::rgb(1.0, 0.0, 0.0),
    )));
    let mut top = shape_item(square, Affine2::translate(45.0, 30.0), DisplayColor::rgb(0.0, 0.0, 1.0));
    top.opacity = Opacity::new(0.5).unwrap();
    list.items.push(shape(top));
    let mut rig = Rig::new(
        &context,
        (80, 60),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let (report, pixels) = rig.render(&list, ABSENT);
    assert_eq!(report.draw_calls, 1, "две фигуры подряд — один вызов рисования");
    let overlap = pixel(&pixels, 80, 38, 30);
    // Синий с прозрачностью 0,5 поверх красного: (0,5 синий + 0,5 красный).
    assert!(
        (overlap[0] - 0.5).abs() < 0.02 && (overlap[2] - 0.5).abs() < 0.02,
        "{overlap:?}"
    );
    let only_red = pixel(&pixels, 80, 15, 30);
    assert!(only_red[0] > 0.95 && only_red[2] < 0.05, "{only_red:?}");
    assert_no_validation_errors(&context);
}
