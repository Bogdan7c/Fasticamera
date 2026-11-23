//! Живые картинки UI (сессия 14h): картинка, посчитанная на GPU, рисуется обычным элементом списка без копии
//! в CPU — паритет с эталоном `shade_image_pixel_with_live`, прямая альфа, отсутствие перевода sRGB→P3,
//! отсутствующая картинка не рисуется и не роняет кадр.

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
use support::mirror::worst_difference;
use support::pictures::{flat_picture, image_item, render_with_images_and_live};
use support::{CLEAR, Rig, assert_no_validation_errors, empty_list, gpu_context_or_skip, pixel, shape, shape_item};
use ui_draw::color::{DisplayColor, DisplayPrimaries, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem, ImageSource, LiveImage, LiveImageTexels};
use ui_draw::geometry::PxSize;
use ui_draw::picture::PictureSize;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_render_vulkan::testing_live::SyntheticLiveImage;
use ui_render_vulkan::{BackdropInput, LiveImages};

const ABSENT: BackdropInput<'static> = BackdropInput::Absent {
    fill: DisplayColor::BLACK,
};

const WIDTH: u32 = 80;
const HEIGHT: u32 = 60;

/// Допуск паритета: как у картинок из памяти (`tests/images.rs`) — 2/255. Источники расхождения те же:
/// фиксированная точка весов билинейной фильтрации сэмплера и округление записи/смешивания в цель Rgba8.
/// Текстура fp16 с узором из значений, точно представимых в fp16, ошибки не добавляет.
const PARITY_TOLERANCE: f32 = 2.0 / 255.0;

/// Допуск ручной проверки «этот пиксель равен числам из формулы»: округление цели Rgba8 (±0,5/255) и
/// смешивания (ещё ±0,5/255) — с запасом 0,01.
const HAND_TOLERANCE: f32 = 0.01;

fn rig_with(context: &gpu_vulkan::context::GpuContext, primaries: DisplayPrimaries) -> Rig {
    Rig::with_config(
        context,
        (WIDTH, HEIGHT),
        SurfaceTransform::Identity,
        ColorTargetFormat::Rgba8Unorm,
        |config| {
            config.display_primaries = primaries;
            config.limits.max_images = 3;
        },
    )
}

fn scope_extent() -> vk::Extent2D {
    vk::Extent2D { width: 16, height: 8 }
}

/// Элемент-прибор: рамка `frame` с центром `center`, `Stretch` и без обрезки.
fn scope_item(frame: (f32, f32), center: (f32, f32), opacity: f32) -> DrawItem {
    DrawItem::Image(ImageItem {
        source: ImageSource::Live(LiveImage::ColorScope),
        transform: Affine2::translate(center.0, center.1),
        size_px: PxSize::new(frame.0, frame.1),
        fit: ImageFit::Stretch,
        clip: ImageClip::None,
        opacity: Opacity::new(opacity).unwrap(),
    })
}

fn list_of(items: Vec<DrawItem>) -> DrawList {
    let mut list = empty_list();
    list.items = items;
    list
}

fn close(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() <= HAND_TOLERANCE
}

#[test]
fn live_image_is_drawn_like_the_reference_with_straight_alpha_and_without_srgb_conversion() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let scope = SyntheticLiveImage::new(&context, scope_extent()).unwrap();
    let entries = [(LiveImage::ColorScope, scope.input())];
    let texels_cpu = scope.texels();
    let texels = LiveImageTexels::new(PictureSize::new(16, 8), &texels_cpu).unwrap();
    // Прибор 48×24 (по 3 px на тексель) в (26..74, 23..47) с прозрачностью 0,8; тот же прибор второй раз,
    // узкий, поверх (общий сэмплер); красная картинка из памяти слева — на P3 её цвет ПЕРЕВОДИТСЯ.
    let red = flat_picture(4, 4, [255, 0, 0, 255]);
    let list = list_of(vec![
        DrawItem::Image(image_item(&red, (16.0, 16.0), Affine2::translate(12.0, 12.0), 1.0)),
        scope_item((48.0, 24.0), (50.0, 35.0), 0.8),
        scope_item((16.0, 8.0), (50.0, 52.0), 1.0),
    ]);

    let mut renders = Vec::new();
    for primaries in [DisplayPrimaries::Srgb, DisplayPrimaries::DisplayP3] {
        let mut rig = rig_with(&context, primaries);
        let (result, pixels) = rig.render_with_live_images(&list, ABSENT, CLEAR, LiveImages::new(&entries));
        let report = result.unwrap();
        assert_eq!(
            (
                report.image_instances,
                report.live_images_drawn,
                report.live_images_missing,
                report.picture_uploads
            ),
            (3, 2, 0, 1),
            "{primaries:?}"
        );
        let counters = rig.renderer.counters();
        assert_eq!((counters.live_images_drawn, counters.live_images_missing), (2, 0));

        let expected = render_with_images_and_live(&list, WIDTH, HEIGHT, CLEAR, primaries, Some(&texels));
        let (worst, at) = worst_difference(&pixels, &expected, WIDTH);
        assert!(
            worst <= PARITY_TOLERANCE,
            "{primaries:?}: наибольшее расхождение {worst} в пикселе {at:?}, допуск {PARITY_TOLERANCE}"
        );

        // Ручная проверка по формулам, не через эталон. Пиксель (27, 24) — центр текселя (0,0): (0; 0; 0,75; 1,0).
        // Прозрачность 0,8: цвет = rgb·0,8 + фон·0,2. Если бы перевод sRGB→P3 применился, синий ушёл бы от 0,75.
        let corner = pixel(&pixels, WIDTH, 27, 24);
        assert!(
            close(corner[0], 0.0 * 0.8 + CLEAR[0] * 0.2)
                && close(corner[1], 0.0 * 0.8 + CLEAR[1] * 0.2)
                && close(corner[2], 0.75 * 0.8 + CLEAR[2] * 0.2),
            "{primaries:?}: {corner:?}"
        );
        // Пиксель (72, 45) — центр текселя (15, 7): (0,9375; 0,875; 0,75; альфа 0,53125 × 0,8 = 0,425) — ПРЯМАЯ альфа.
        let far = pixel(&pixels, WIDTH, 72, 45);
        let alpha = 0.53125 * 0.8;
        assert!(
            close(far[0], 0.9375 * alpha + CLEAR[0] * (1.0 - alpha))
                && close(far[1], 0.875 * alpha + CLEAR[1] * (1.0 - alpha))
                && close(far[2], 0.75 * alpha + CLEAR[2] * (1.0 - alpha)),
            "{primaries:?}: {far:?}"
        );
        renders.push(pixels);
        drop(rig);
    }

    // Живые пиксели от основных цветов экрана не зависят (прибор уже посчитан в цвете экрана).
    let (live_difference, at) = region_difference(&renders[0], &renders[1], (26, 23, 74, 59));
    assert!(
        live_difference <= PARITY_TOLERANCE,
        "прибор на sRGB и P3 разный: {live_difference} в {at:?}"
    );
    // А картинка из памяти — зависит: красный sRGB на P3 перестаёт быть (1, 0, 0).
    let red_srgb = pixel(&renders[0], WIDTH, 12, 12);
    let red_p3 = pixel(&renders[1], WIDTH, 12, 12);
    assert!(red_srgb[0] > 0.99 && red_srgb[1] < 0.01, "{red_srgb:?}");
    assert!(red_p3[0] < 0.94 && red_p3[1] > 0.1, "{red_p3:?}");

    drop(scope);
    assert_eq!(
        context.live_objects(),
        baseline,
        "рендер не удерживает ресурсы живой картинки"
    );
    assert_no_validation_errors(&context);
}

/// Наибольшее расхождение по RGB внутри прямоугольника `(x0, y0, x1, y1)` (правая и нижняя границы исключены).
fn region_difference(a: &[[f32; 4]], b: &[[f32; 4]], region: (u32, u32, u32, u32)) -> (f32, (u32, u32)) {
    let mut worst = (0.0_f32, (region.0, region.1));
    for y in region.1..region.3 {
        for x in region.0..region.2 {
            let (left, right) = (pixel(a, WIDTH, x, y), pixel(b, WIDTH, x, y));
            let diff = (0..3).map(|c| (left[c] - right[c]).abs()).fold(0.0, f32::max);
            if diff > worst.0 {
                worst = (diff, (x, y));
            }
        }
    }
    worst
}

#[test]
fn live_image_with_cover_and_circle_matches_the_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let scope = SyntheticLiveImage::new(&context, scope_extent()).unwrap();
    let entries = [(LiveImage::ColorScope, scope.input())];
    let texels_cpu = scope.texels();
    let texels = LiveImageTexels::new(PictureSize::new(16, 8), &texels_cpu).unwrap();
    // Рамка 30×30 над источником 16×8: Cover обрезает по ширине, круг — углы.
    let list = list_of(vec![DrawItem::Image(ImageItem {
        source: ImageSource::Live(LiveImage::ColorScope),
        transform: Affine2::rotate(0.3).then(Affine2::translate(40.0, 30.0)),
        size_px: PxSize::new(30.0, 30.0),
        fit: ImageFit::Cover,
        clip: ImageClip::Circle,
        opacity: Opacity::OPAQUE,
    })]);
    let mut rig = rig_with(&context, DisplayPrimaries::DisplayP3);
    let (result, pixels) = rig.render_with_live_images(&list, ABSENT, CLEAR, LiveImages::new(&entries));
    assert_eq!(result.unwrap().live_images_drawn, 1);
    let expected = render_with_images_and_live(&list, WIDTH, HEIGHT, CLEAR, DisplayPrimaries::DisplayP3, Some(&texels));
    let (worst, at) = worst_difference(&pixels, &expected, WIDTH);
    assert!(worst <= PARITY_TOLERANCE, "расхождение {worst} в {at:?}");
    let centre = pixel(&pixels, WIDTH, 40, 30);
    assert!(
        (centre[2] - CLEAR[2]).abs() > 0.05,
        "картинка действительно нарисована: {centre:?}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn absent_live_image_draws_nothing_counts_the_miss_and_does_not_fail_the_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = rig_with(&context, DisplayPrimaries::DisplayP3);
    let disc = || {
        shape(shape_item(
            ShapeGeometry::Circle { diameter: 10.0 },
            Affine2::translate(10.0, 10.0),
            DisplayColor::WHITE,
        ))
    };
    let with_scope = list_of(vec![disc(), scope_item((48.0, 24.0), (50.0, 35.0), 1.0)]);
    let without_scope = list_of(vec![disc()]);

    let (result, pixels) = rig.render_with_live_images(&with_scope, ABSENT, CLEAR, LiveImages::none());
    let report = result.expect("отсутствие живой картинки — штатное состояние, кадр не падает");
    assert_eq!(
        (
            report.image_instances,
            report.live_images_drawn,
            report.live_images_missing
        ),
        (0, 0, 1)
    );
    assert_eq!(report.shape_instances, 1, "соседние элементы рисуются как обычно");
    let (_, reference) = rig.render(&without_scope, ABSENT);
    assert_eq!(
        pixels, reference,
        "без живой картинки на кадре элемент не оставляет ни одного пикселя"
    );

    // Картинка есть в таблице, но другая запись пуста: тоже «нет» (пустая таблица и таблица без ключа равны).
    let counters = rig.renderer.counters();
    assert_eq!(
        (
            counters.live_images_drawn,
            counters.live_images_missing,
            counters.images_drawn,
            counters.frames_failed
        ),
        (0, 1, 0, 0)
    );
    assert_no_validation_errors(&context);
}

#[test]
fn live_image_appearing_on_a_later_frame_is_drawn_from_that_frame_on() {
    let Some(context) = gpu_context_or_skip() else { return };
    let scope = SyntheticLiveImage::new(&context, scope_extent()).unwrap();
    let entries = [(LiveImage::ColorScope, scope.input())];
    let mut rig = rig_with(&context, DisplayPrimaries::Srgb);
    let list = list_of(vec![scope_item((48.0, 24.0), (50.0, 35.0), 1.0)]);

    let (first, hidden) = rig.render_with_live_images(&list, ABSENT, CLEAR, LiveImages::none());
    let (second, shown) = rig.render_with_live_images(&list, ABSENT, CLEAR, LiveImages::new(&entries));
    assert_eq!(
        (first.unwrap().live_images_missing, second.unwrap().live_images_drawn),
        (1, 1)
    );
    assert_ne!(hidden, shown, "после появления картинки кадр изменился");
    // Прибор исчез снова — следов не осталось (никакого «залипания» прошлой картинки в привязках слота).
    let (_, hidden_again) = rig.render_with_live_images(&list, ABSENT, CLEAR, LiveImages::none());
    assert_eq!(hidden, hidden_again);
    let counters = rig.renderer.counters();
    assert_eq!((counters.live_images_drawn, counters.live_images_missing), (1, 2));
    assert_no_validation_errors(&context);
}
