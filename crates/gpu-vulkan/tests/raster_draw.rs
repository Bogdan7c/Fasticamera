//! Функциональные тесты растрового набора (сессия 11) на реальном GPU (RADV, полная validation):
//! инстансинг прямоугольников из SSBO, смешивание, два прохода в одну цель, scissor.
//!
//! Каждый тест читает пиксели цели (readback) и сверяет их с ожидаемыми значениями; сломанный индекс
//! экземпляра, режим смешивания, барьер между проходами или scissor дают другие числа либо ошибку
//! validation (любая ошибка validation = провал).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;
mod raster_support;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::color_target::{ColorTargetFormat, ColorTargetPreviousUse, ContentOnPrepare};
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, BlendMode, DrawCall, RasterPass, record_between_color_passes};
use raster_support::{
    BLACK, BLUE, GREEN, RED, RectInstance, assert_pixel_near, color_target, instances_write, pixel_at, rect_buffer,
    rect_pipeline, target_size_push,
};

const FORMATS: [ColorTargetFormat; 3] = [
    ColorTargetFormat::Rgba8Unorm,
    ColorTargetFormat::A2B10G10R10Unorm,
    ColorTargetFormat::Rgba16Float,
];

fn full_draw(instance_count: u32) -> DrawCall {
    DrawCall {
        vertices_per_instance: 6,
        instance_count,
        scissor: None,
    }
}

#[test]
fn many_rectangles_from_one_draw_land_in_their_own_places_with_their_own_colors() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let clear = [0.1, 0.2, 0.3, 1.0];
    // Непересекающиеся прямоугольники разного цвета и размера: перепутанный индекс экземпляра или
    // оси дадут не те цвета/места.
    let rects = [
        RectInstance::new(2, 2, 6, 4, RED),
        RectInstance::new(10, 1, 4, 8, GREEN),
        RectInstance::new(16, 10, 6, 5, BLUE),
        RectInstance::new(0, 12, 5, 4, [1.0, 1.0, 0.0, 1.0]),
        RectInstance::new(8, 9, 3, 3, [1.0, 0.0, 1.0, 1.0]),
    ];
    for format in FORMATS {
        let target = color_target(&context, 24, 16, format);
        let buffer = rect_buffer(&context, &rects);
        let mut pipeline = rect_pipeline(&context, target.format(), BlendMode::Opaque, 1);
        pipeline.bind(0, &[instances_write(&buffer)]).unwrap();
        run_one_shot_blocking(&context, |command_buffer| {
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            let mut pass = RasterPass::begin(
                &context,
                command_buffer,
                target.attachment(),
                AttachmentLoad::Clear(clear),
            )?;
            pass.draw(
                &pipeline,
                0,
                &target_size_push(target.extent()),
                full_draw(rects.len() as u32),
            )?;
            pass.end();
            Ok(())
        })
        .unwrap();
        let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
        assert_eq!(pixels.len(), 24 * 16);
        for y in 0..16 {
            for x in 0..24 {
                let expected = rects
                    .iter()
                    .find(|rect| rect.contains(x, y))
                    .map_or(clear, |rect| rect.color);
                assert_pixel_near(
                    pixel_at(&pixels, 24, x, y),
                    expected,
                    format,
                    &format!("{format:?} ({x},{y})"),
                );
            }
        }
    }
    assert_eq!(context.live_objects(), baseline, "все объекты возвращены");
    assert_no_validation_errors(&context);
}

#[test]
fn premultiplied_alpha_blends_over_preserved_content_and_opaque_replaces_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let destination = [0.2, 0.4, 0.6, 1.0];
    // Premultiplied: цвет уже умножен на альфу 0.5.
    let translucent = [0.25, 0.0, 0.125, 0.5];
    let replacing = [0.9, 0.1, 0.5, 0.5];
    // src + dst·(1 − 0.5): (0.35, 0.2, 0.425, 1.0).
    let blended_expected = [0.35, 0.2, 0.425, 1.0];
    for format in [ColorTargetFormat::Rgba8Unorm, ColorTargetFormat::Rgba16Float] {
        let target = color_target(&context, 8, 8, format);
        let blend_buffer = rect_buffer(&context, &[RectInstance::new(0, 0, 4, 8, translucent)]);
        let opaque_buffer = rect_buffer(&context, &[RectInstance::new(4, 0, 4, 8, replacing)]);
        let mut blend_pipeline = rect_pipeline(&context, target.format(), BlendMode::PremultipliedAlpha, 1);
        let mut opaque_pipeline = rect_pipeline(&context, target.format(), BlendMode::Opaque, 1);
        blend_pipeline.bind(0, &[instances_write(&blend_buffer)]).unwrap();
        opaque_pipeline.bind(0, &[instances_write(&opaque_buffer)]).unwrap();
        run_one_shot_blocking(&context, |command_buffer| {
            target.record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Discard,
            )?;
            // Первый проход заливает «загруженное» содержимое, второй (Preserve) смешивает поверх него.
            RasterPass::begin(
                &context,
                command_buffer,
                target.attachment(),
                AttachmentLoad::Clear(destination),
            )?
            .end();
            record_between_color_passes(&context, command_buffer);
            let mut pass = RasterPass::begin(&context, command_buffer, target.attachment(), AttachmentLoad::Preserve)?;
            let push = target_size_push(target.extent());
            pass.draw(&blend_pipeline, 0, &push, full_draw(1))?;
            pass.draw(&opaque_pipeline, 0, &push, full_draw(1))?;
            pass.end();
            Ok(())
        })
        .unwrap();
        let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
        for y in 0..8 {
            for x in 0..4 {
                assert_pixel_near(
                    pixel_at(&pixels, 8, x, y),
                    blended_expected,
                    format,
                    &format!("{format:?} blend ({x},{y})"),
                );
            }
            for x in 4..8 {
                assert_pixel_near(
                    pixel_at(&pixels, 8, x, y),
                    replacing,
                    format,
                    &format!("{format:?} opaque ({x},{y})"),
                );
            }
        }
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn second_pass_into_the_same_target_keeps_the_first_pass_content() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let format = ColorTargetFormat::A2B10G10R10Unorm;
    let target = color_target(&context, 16, 16, format);
    let first = rect_buffer(&context, &[RectInstance::new(2, 2, 10, 10, RED)]);
    let second = rect_buffer(&context, &[RectInstance::new(8, 8, 6, 6, BLUE)]);
    // Один конвейер, два слота: каждому проходу — свой буфер экземпляров.
    let mut pipeline = rect_pipeline(&context, target.format(), BlendMode::Opaque, 2);
    pipeline.bind(0, &[instances_write(&first)]).unwrap();
    pipeline.bind(1, &[instances_write(&second)]).unwrap();
    run_one_shot_blocking(&context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        let push = target_size_push(target.extent());
        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(BLACK),
        )?;
        pass.draw(&pipeline, 0, &push, full_draw(1))?;
        pass.end();
        record_between_color_passes(&context, command_buffer);
        let mut pass = RasterPass::begin(&context, command_buffer, target.attachment(), AttachmentLoad::Preserve)?;
        pass.draw(&pipeline, 1, &push, full_draw(1))?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    let first_rect = RectInstance::new(2, 2, 10, 10, RED);
    let second_rect = RectInstance::new(8, 8, 6, 6, BLUE);
    for y in 0..16 {
        for x in 0..16 {
            let expected = if second_rect.contains(x, y) {
                BLUE
            } else if first_rect.contains(x, y) {
                RED
            } else {
                BLACK
            };
            assert_pixel_near(pixel_at(&pixels, 16, x, y), expected, format, &format!("({x},{y})"));
        }
    }
    drop((pipeline, first, second, target));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn scissor_clips_one_draw_and_does_not_leak_into_the_next() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let format = ColorTargetFormat::Rgba8Unorm;
    let gray = [0.5, 0.5, 0.5, 1.0];
    let target = color_target(&context, 16, 16, format);
    let big_green = rect_buffer(&context, &[RectInstance::new(0, 0, 16, 16, GREEN)]);
    let small_red = rect_buffer(&context, &[RectInstance::new(0, 0, 2, 2, RED)]);
    let mut clipped = rect_pipeline(&context, target.format(), BlendMode::Opaque, 1);
    let mut unclipped = rect_pipeline(&context, target.format(), BlendMode::Opaque, 1);
    clipped.bind(0, &[instances_write(&big_green)]).unwrap();
    unclipped.bind(0, &[instances_write(&small_red)]).unwrap();
    let scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 4, y: 5 },
        extent: vk::Extent2D { width: 8, height: 6 },
    };
    run_one_shot_blocking(&context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        let push = target_size_push(target.extent());
        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(gray),
        )?;
        pass.draw(
            &clipped,
            0,
            &push,
            DrawCall {
                vertices_per_instance: 6,
                instance_count: 1,
                scissor: Some(scissor),
            },
        )?;
        // Следующий draw без scissor: красный угол вне прежнего отсечения обязан нарисоваться.
        pass.draw(&unclipped, 0, &push, full_draw(1))?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for y in 0..16 {
        for x in 0..16 {
            let inside_scissor = (4..12).contains(&x) && (5..11).contains(&y);
            let expected = if x < 2 && y < 2 {
                RED
            } else if inside_scissor {
                GREEN
            } else {
                gray
            };
            assert_pixel_near(pixel_at(&pixels, 16, x, y), expected, format, &format!("({x},{y})"));
        }
    }
    drop((clipped, unclipped, big_green, small_red, target));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
