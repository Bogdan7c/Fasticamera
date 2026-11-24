//! Дайджест выхода работы стенда насыщения на реальном GPU (RADV, полная validation).
//!
//! Что проверяется (результат, а не вызов): отпечаток цветовой цели равен отпечатку заведомо известных байт
//! (значит читаются именно пиксели, и хэшируются именно они), одинаков при повторном чтении, меняется вместе с
//! картинкой, не меняет раскладку и содержимое читаемого; буфер счётчиков хэшируется до границы сетки;
//! отсутствующий выход и ошибки — типизированные.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;
mod raster_support;
mod saturation_support;

use std::time::Duration;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::color_target::{ColorTarget, ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::DeviceBuffer;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, RasterPass};
use gpu_vulkan::saturation::{
    CancelToken, DigestError, Fnv1a64, SaturationBench, SaturationConfig, digest_color_target,
    digest_device_buffer_prefix, digest_rgba8_bytes, digest_storage_image,
};
use gpu_vulkan::storage_image::StorageImagePreviousUse;
use gpu_vulkan::swapchain::FramesInFlight;
use raster_support::{BLUE, RED, color_target};
use saturation_support::PatternWorkload;

/// Заливает цель одним цветом и оставляет её в состоянии «нарисована».
fn cleared_target(context: &GpuContext, width: u32, height: u32, color: [f32; 4]) -> ColorTarget {
    let target = color_target(context, width, height, ColorTargetFormat::Rgba8Unorm);
    run_one_shot_blocking(context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        RasterPass::begin(
            context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(color),
        )?
        .end();
        Ok(())
    })
    .unwrap();
    target
}

#[test]
fn color_target_digest_equals_the_digest_of_the_known_pixel_bytes_and_follows_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let (width, height) = (6_u32, 3_u32);
    let red = cleared_target(&context, width, height, RED);
    let blue = cleared_target(&context, width, height, BLUE);

    let digest = digest_color_target(&red, ColorTargetPreviousUse::Rendered).unwrap();
    // Независимый эталон: те же пиксели, записанные байтами (красный непрозрачный, RGBA8).
    let known_bytes: Vec<u8> = (0..width * height).flat_map(|_| [255, 0, 0, 255]).collect();
    assert_eq!(
        digest,
        digest_rgba8_bytes(vk::Extent2D { width, height }, &known_bytes),
        "читаются и хэшируются именно пиксели цели"
    );
    // Повторное чтение даёт тот же отпечаток, а читаемое не меняется (раскладка восстановлена).
    assert_eq!(
        digest_color_target(&red, ColorTargetPreviousUse::Rendered).unwrap(),
        digest
    );
    let after = read_color_target_rgba(&red, ColorTargetPreviousUse::Rendered).unwrap();
    assert!(after.iter().all(|pixel| *pixel == RED), "содержимое цели не тронуто");
    // Другая картинка — другой отпечаток; другая форма тех же байт — тоже.
    assert_ne!(
        digest_color_target(&blue, ColorTargetPreviousUse::Rendered).unwrap(),
        digest
    );
    assert_ne!(
        digest_rgba8_bytes(
            vk::Extent2D {
                width: height,
                height: width
            },
            &known_bytes
        ),
        digest_rgba8_bytes(vk::Extent2D { width, height }, &known_bytes),
        "форма входит в отпечаток"
    );

    drop((red, blue));
    assert_eq!(context.live_objects(), baseline, "readback не оставил объектов Vulkan");
    assert_no_validation_errors(&context);
}

#[test]
fn a_target_that_was_never_drawn_gives_a_typed_error_not_a_digest_of_garbage() {
    let Some(context) = gpu_context_or_skip() else { return };
    let target = color_target(&context, 4, 4, ColorTargetFormat::Rgba8Unorm);
    let error = digest_color_target(&target, ColorTargetPreviousUse::NotUsedYet).unwrap_err();
    assert!(matches!(error, DigestError::Gpu(_)), "{error:?}");
    assert!(error.to_string().contains("до первой записи"), "{error}");
    drop(target);
    assert_no_validation_errors(&context);
}

#[test]
fn storage_image_digest_is_stable_covers_only_the_content_and_does_not_disturb_the_image() {
    let Some(context) = gpu_context_or_skip() else { return };
    let side = 32_u32;
    let mut work = PatternWorkload::new(&context, side, 2);
    let mut bench = SaturationBench::new(
        &context,
        SaturationConfig {
            frames_in_flight: FramesInFlight::new(2).unwrap(),
            warmup: Duration::from_millis(20),
            measure: Duration::from_millis(60),
            timeline_step: Duration::from_millis(20),
            fence_timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    bench.run(&mut work, &CancelToken::new()).unwrap();
    drop(bench);
    context.wait_idle().unwrap();

    // Отпечаток следует за пикселями: после каждого нового прогона (синий канал несёт номер последнего кадра) равенство
    // пикселей совпадает с равенством отпечатков, и обе ситуации встретились.
    let mut states = vec![(
        read_storage_image_rgba(work.image(), work.image_use_after_frames()).unwrap(),
        digest_storage_image(
            work.image(),
            work.image_use_after_frames(),
            vk::Extent2D {
                width: side,
                height: side,
            },
        )
        .unwrap(),
    )];
    for measure_ms in [30, 45, 70, 25, 55, 90] {
        let mut bench = SaturationBench::new(
            &context,
            SaturationConfig {
                frames_in_flight: FramesInFlight::new(2).unwrap(),
                warmup: Duration::ZERO,
                measure: Duration::from_millis(measure_ms),
                timeline_step: Duration::from_millis(measure_ms),
                fence_timeout: Duration::from_secs(10),
            },
        )
        .unwrap();
        bench.run(&mut work, &CancelToken::new()).unwrap();
        drop(bench);
        context.wait_idle().unwrap();
        states.push((
            read_storage_image_rgba(work.image(), work.image_use_after_frames()).unwrap(),
            digest_storage_image(
                work.image(),
                work.image_use_after_frames(),
                vk::Extent2D {
                    width: side,
                    height: side,
                },
            )
            .unwrap(),
        ));
    }
    for (first, second) in states
        .iter()
        .flat_map(|first| states.iter().map(move |second| (first, second)))
    {
        assert_eq!(
            first.0 == second.0,
            first.1 == second.1,
            "равенство отпечатков должно совпадать с равенством пикселей"
        );
    }
    assert!(
        states.iter().any(|state| state.0 != states[0].0),
        "картинка менялась между прогонами: проверка не пустая"
    );

    let image = work.image();
    let used = work.image_use_after_frames();
    let before = read_storage_image_rgba(image, used).unwrap();
    let full = vk::Extent2D {
        width: side,
        height: side,
    };
    let half = vk::Extent2D {
        width: side / 2,
        height: side / 2,
    };
    let digest_full = digest_storage_image(image, used, full).unwrap();
    assert_eq!(
        digest_storage_image(image, used, full).unwrap(),
        digest_full,
        "повторное чтение — тот же отпечаток"
    );
    // Градиент координат: левая верхняя четверть содержимого отличается от всей картинки.
    assert_ne!(digest_storage_image(image, used, half).unwrap(), digest_full);
    assert_eq!(
        read_storage_image_rgba(image, used).unwrap(),
        before,
        "чтение дайджеста не меняет картинку и её раскладку"
    );
    // Содержимое больше ёмкости — ошибка вызывающего, а не отпечаток обрезка.
    let too_big = vk::Extent2D {
        width: side + 1,
        height: side,
    };
    assert!(matches!(
        digest_storage_image(image, used, too_big).unwrap_err(),
        DigestError::ContentOutsideImage { .. }
    ));
    // Картинка, в которую ещё не писали: нечего читать.
    assert!(matches!(
        digest_storage_image(image, StorageImagePreviousUse::NotUsedYet, full).unwrap_err(),
        DigestError::Gpu(_)
    ));
    drop(work);
    assert_no_validation_errors(&context);
}

#[test]
fn device_buffer_digest_covers_the_requested_prefix_only_and_rejects_a_prefix_longer_than_the_buffer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let first = DeviceBuffer::new(&context, 32).unwrap();
    let second = DeviceBuffer::new(&context, 32).unwrap();
    let mut bytes: Vec<u8> = (0..32).collect();
    first.upload_blocking(&bytes).unwrap();
    // Хвост за границей сетки отличается, начало то же.
    bytes[20] ^= 0xff;
    second.upload_blocking(&bytes).unwrap();

    let prefix_first = digest_device_buffer_prefix(&first, 16).unwrap();
    assert_eq!(
        prefix_first,
        digest_device_buffer_prefix(&second, 16).unwrap(),
        "байты за границей сетки в отпечаток не входят"
    );
    assert_ne!(
        digest_device_buffer_prefix(&first, 24).unwrap(),
        digest_device_buffer_prefix(&second, 24).unwrap(),
        "отличие внутри границы видно"
    );
    // Эталон: длина префикса и сами байты.
    let mut oracle = Fnv1a64::new();
    oracle.update(&16_u64.to_le_bytes());
    oracle.update(&(0..16).collect::<Vec<u8>>());
    assert_eq!(prefix_first, oracle.finish());
    assert!(matches!(
        digest_device_buffer_prefix(&first, 64).unwrap_err(),
        DigestError::BufferTooShort {
            requested: 64,
            read: 32
        }
    ));
    drop((first, second));
    assert_no_validation_errors(&context);
}
