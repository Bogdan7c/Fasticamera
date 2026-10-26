//! Сквозные тесты: тестовый паттерн реально рисуется на GPU и читается обратно побитно.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::offscreen::{OffscreenTarget, Rgba8Image};
use gpu_vulkan::test_pattern::{SurfaceTransform, TestPattern, expected_pixel_with_transform};

/// Нечётный непрямоугольный размер: ловит ошибки округления, перепутанные оси и выравнивание строк.
const WIDTH: u32 = 97;
const HEIGHT: u32 = 61;
/// Кадры: ноль, один бит, несколько битов, старший бит метки.
const FRAMES: [u32; 4] = [0, 1, 0b0101_0110, 0b1000_0001];

fn render_pattern(
    pattern: &TestPattern,
    target: &mut OffscreenTarget,
    frame: u32,
    transform: SurfaceTransform,
) -> Result<Rgba8Image, GpuError> {
    target.render(|command_buffer, attachment| pattern.record(command_buffer, attachment, frame, transform))?;
    target.readback()
}

/// Сравнивает КАЖДЫЙ пиксель с CPU-эталоном; при расхождении сообщение содержит координаты.
fn assert_every_pixel_matches(image: &Rgba8Image, frame: u32, transform: SurfaceTransform) {
    let extent = vk::Extent2D {
        width: image.width,
        height: image.height,
    };
    assert_eq!((image.width, image.height), (WIDTH, HEIGHT));
    assert_eq!(image.pixels.len(), (WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let expected = expected_pixel_with_transform(x, y, extent, frame, transform).expect("поддержанный поворот");
            let actual = image.pixel(x, y).expect("пиксель внутри картинки");
            assert_eq!(actual, expected, "{transform:?}, кадр {frame}, пиксель ({x},{y})");
        }
    }
}

#[test]
fn pattern_is_bit_exact_for_every_pixel_frame_and_rotation() {
    let Some(context) = gpu_context_or_skip() else { return };
    let pattern = TestPattern::new(&context, OffscreenTarget::FORMAT).expect("пайплайн паттерна");
    let mut target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("цель");

    for transform in SurfaceTransform::ROTATIONS {
        for frame in FRAMES {
            let image = render_pattern(&pattern, &mut target, frame, transform).expect("рендер и readback");
            assert_every_pixel_matches(&image, frame, transform);
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn rotation_actually_changes_the_picture_and_frame_changes_marker() {
    let Some(context) = gpu_context_or_skip() else { return };
    let pattern = TestPattern::new(&context, OffscreenTarget::FORMAT).expect("пайплайн");
    let mut target = OffscreenTarget::new(&context, WIDTH, HEIGHT).expect("цель");

    let upright = render_pattern(&pattern, &mut target, 3, SurfaceTransform::Identity).unwrap();
    let rotated = render_pattern(&pattern, &mut target, 3, SurfaceTransform::Rotate180).unwrap();
    let other_frame = render_pattern(&pattern, &mut target, 4, SurfaceTransform::Identity).unwrap();
    assert_ne!(upright.pixels, rotated.pixels, "поворот не повлиял на картинку");
    assert_ne!(upright.pixels, other_frame.pixels, "номер кадра не повлиял на картинку");
    // Метка кадра 3 (биты 0 и 1): в верхней строке левая клетка белая, правая чёрная.
    assert_eq!(upright.pixel(0, 0), Some([255, 255, 255, 255]));
    assert_eq!(upright.pixel(WIDTH - 1, 0), Some([0, 0, 0, 255]));
    assert_no_validation_errors(&context);
}

#[test]
fn mirrored_transform_is_reported_unsupported_and_leaves_target_unrendered() {
    let Some(context) = gpu_context_or_skip() else { return };
    let pattern = TestPattern::new(&context, OffscreenTarget::FORMAT).unwrap();
    let mut target = OffscreenTarget::new(&context, WIDTH, HEIGHT).unwrap();
    let error = render_pattern(&pattern, &mut target, 0, SurfaceTransform::HorizontalMirror).unwrap_err();
    assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
    // Неудачный рендер не должен делать readback возможным.
    let readback_error = target.readback().unwrap_err();
    assert!(
        matches!(readback_error, GpuError::InvalidState { .. }),
        "{readback_error:?}"
    );
    // И цель остаётся рабочей.
    let image = render_pattern(&pattern, &mut target, 1, SurfaceTransform::Identity).unwrap();
    assert_every_pixel_matches(&image, 1, SurfaceTransform::Identity);
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_target_sizes_are_rejected_without_creating_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    for (width, height) in [(0, 10), (10, 0), (u32::MAX, 10)] {
        let error = OffscreenTarget::new(&context, width, height)
            .err()
            .expect("ошибка размера");
        assert!(
            matches!(error, GpuError::InvalidArgument { .. }),
            "{width}x{height}: {error:?}"
        );
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
