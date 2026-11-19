//! Поворот цели показа: список рисования в логических px попадает в правильное физическое место для каждого
//! `SurfaceTransform`. Ожидание строится НЕЗАВИСИМО от формул шейдера: физическая картинка поворачивается по часовой
//! стрелке на `k` четвертей, как это делает дисплей (определение `VK_SURFACE_TRANSFORM_ROTATE_90`), и обязана совпасть
//! с кадром, нарисованным без поворота.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod support;

use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::test_pattern::SurfaceTransform;
use support::mirror::worst_difference;
use support::{CLEAR, Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, pixel, shape, shape_item};
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, TextItem};
use ui_draw::geometry::PxSize;
use ui_draw::shape::{CornerSpec, ShapeGeometry};
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};
use ui_draw::transform::Affine2;
use ui_render_vulkan::BackdropInput;

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};
const LOGICAL: (u32, u32) = (120, 200);

/// Поворот картинки на 90° по часовой стрелке: пиксель (x, y) картинки W×H уходит в (H−1−y, x) картинки H×W.
fn rotate_clockwise(pixels: &[[f32; 4]], width: u32, height: u32) -> (Vec<[f32; 4]>, u32, u32) {
    let mut rotated = vec![[0.0; 4]; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            rotated[(x * height + (height - 1 - y)) as usize] = pixels[(y * width + x) as usize];
        }
    }
    (rotated, height, width)
}

fn scene() -> ui_draw::draw_list::DrawList {
    let mut list = empty_list();
    // Прямоугольник в логическом ЛЕВОМ ВЕРХНЕМ углу и круг в правом нижнем: асимметрия, которую не спутать при повороте.
    list.items.push(shape(shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(40.0, 20.0),
            corner: CornerSpec::continuous(6.0),
        },
        Affine2::translate(30.0, 20.0),
        DisplayColor::rgb(0.9, 0.2, 0.2),
    )));
    list.items.push(shape(shape_item(
        ShapeGeometry::Circle { diameter: 30.0 },
        Affine2::translate(85.0, 170.0),
        DisplayColor::rgb(0.2, 0.8, 0.3),
    )));
    let style = TextStyle {
        face: FontFace::Semibold,
        size_px: 32.0,
        tracking_em: 0.0,
        figures: Figures::Proportional,
    };
    list.items.push(DrawItem::Text(TextItem {
        run: layout_line("PH", &style).unwrap(),
        transform: Affine2::translate(20.0, 110.0),
        color: DisplayColor::WHITE,
        opacity: Opacity::OPAQUE,
    }));
    list
}

#[test]
fn every_surface_rotation_matches_the_unrotated_frame_rotated_back_by_the_display() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let list = scene();
    let mut reference_rig = Rig::new(
        &context,
        LOGICAL,
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
    );
    let (_, logical_pixels) = reference_rig.render(&list, ABSENT);
    // Страховка: прямоугольник нарисован в логическом левом верхнем углу.
    assert!(pixel(&logical_pixels, LOGICAL.0, 30, 20)[0] > 0.8);
    assert!((pixel(&logical_pixels, LOGICAL.0, 100, 10)[0] - CLEAR[0]).abs() < 0.01);

    for (transform, quarter_turns) in [
        (SurfaceTransform::Rotate90, 1),
        (SurfaceTransform::Rotate180, 2),
        (SurfaceTransform::Rotate270, 3),
    ] {
        let mut rig = Rig::new(&context, LOGICAL, transform, ColorTargetFormat::Rgba8Unorm);
        let (_, physical) = rig.render(&list, ABSENT);
        let (mut shown, mut width, mut height) = (physical, rig.target.extent().width, rig.target.extent().height);
        for _ in 0..quarter_turns {
            (shown, width, height) = rotate_clockwise(&shown, width, height);
        }
        assert_eq!(
            (width, height),
            LOGICAL,
            "{transform:?}: после поворота дисплея размер логический"
        );
        // Текст сглаживается по производным, у повёрнутых целей они переставлены, поэтому допуск шире, чем у фигур;
        // но ошибка позиции даже в 1 px даёт расхождение около 0,5, так что 0,08 различает.
        let (worst, at) = worst_difference(&shown, &logical_pixels, width);
        assert!(
            worst < 0.08,
            "{transform:?}: после поворота дисплеем расхождение {worst} в {at:?}"
        );
    }
    drop((reference_rig, list));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn rect_in_logical_top_left_lands_in_the_expected_physical_corner() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut list = empty_list();
    list.items.push(shape(shape_item(
        ShapeGeometry::RoundedRect {
            size: PxSize::new(20.0, 10.0),
            corner: CornerSpec::circular(0.0),
        },
        Affine2::translate(12.0, 7.0),
        DisplayColor::WHITE,
    )));
    // Физическая цель 100×60 при логических 60×100 (портрет) или 100×60... размеры берём из LOGICAL = (60, 100).
    let logical = (60_u32, 100_u32);
    // Ожидаемый центр прямоугольника в физической цели (логический центр (12, 7), физический размер Wp×Hp).
    let cases = [
        (SurfaceTransform::Identity, (12, 7)),
        (SurfaceTransform::Rotate90, (7, 60 - 12)),
        (SurfaceTransform::Rotate180, (60 - 12, 100 - 7)),
        (SurfaceTransform::Rotate270, (100 - 7, 12)),
    ];
    for (transform, (want_x, want_y)) in cases {
        let mut rig = Rig::new(&context, logical, transform, ColorTargetFormat::Rgba8Unorm);
        let (_, pixels) = rig.render(&list, ABSENT);
        let width = rig.target.extent().width;
        assert!(
            pixel(&pixels, width, want_x, want_y)[0] > 0.9,
            "{transform:?}: центр прямоугольника ожидался в ({want_x},{want_y})"
        );
        // И белые пиксели лежат только вокруг этого центра (в пределах половины большей стороны).
        let mut count = 0;
        for (index, value) in pixels.iter().enumerate() {
            if value[0] > 0.9 {
                count += 1;
                let (x, y) = (index as u32 % width, index as u32 / width);
                assert!(
                    x.abs_diff(want_x) <= 10 && y.abs_diff(want_y) <= 10,
                    "{transform:?}: лишний белый пиксель ({x},{y})"
                );
            }
        }
        assert_eq!(count, 200, "{transform:?}: площадь прямоугольника 20×10 сохраняется");
    }
    assert_no_validation_errors(&context);
}
