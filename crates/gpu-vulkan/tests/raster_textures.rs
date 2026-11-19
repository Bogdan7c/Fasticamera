//! Тесты текстур растрового набора (сессия 11) на реальном GPU (RADV, полная validation):
//! загруженная `SampledTexture` читается фрагментным шейдером, а нарисованная `ColorTarget` читается
//! вторым проходом (ступень пирамиды размытия фона).

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
use gpu_vulkan::compute::{BindingWrite, DescriptorResource};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, BlendMode, DrawCall, RasterPass};
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use raster_support::{
    BLACK, BLUE, GREEN, RED, RectInstance, WHITE, assert_pixel_near, color_target, instances_write, pixel_at,
    rect_buffer, rect_pipeline, sample_pipeline, target_size_push,
};

const FULLSCREEN: DrawCall = DrawCall {
    vertices_per_instance: 3,
    instance_count: 1,
    scissor: None,
};

#[test]
fn uploaded_r8_texture_is_read_by_the_fragment_shader_at_texel_centers() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    // Неквадратная 8x4: перепутанные оси или строки дадут другой порядок значений.
    let (width, height) = (8_u32, 4_u32);
    let texels: Vec<u8> = (0..width * height).map(|index| (index * 7 + 3) as u8).collect();
    let texture = SampledTexture::upload(
        &context,
        &TextureDesc {
            extent: vk::Extent2D { width, height },
            format: TextureFormat::R8Unorm,
            label: "unit-atlas",
        },
        &texels,
    )
    .unwrap();
    assert_eq!(texture.extent(), vk::Extent2D { width, height });
    assert_eq!(texture.format(), TextureFormat::R8Unorm);

    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, width, height, format);
    let mut pipeline = sample_pipeline(&context, target.format());
    pipeline
        .bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: texture.read_only_binding(),
            }],
        )
        .unwrap();
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
            AttachmentLoad::Clear(BLACK),
        )?;
        pass.draw(&pipeline, 0, &target_size_push(target.extent()), FULLSCREEN)?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for y in 0..height {
        for x in 0..width {
            let value = f32::from(texels[(y * width + x) as usize]) / 255.0;
            // R8: красный канал = значение текселя, зелёный и синий 0, альфа 1.
            assert_pixel_near(
                pixel_at(&pixels, width, x, y),
                [value, 0.0, 0.0, 1.0],
                format,
                &format!("тексель ({x},{y})"),
            );
        }
    }
    drop((pipeline, target, texture));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn rgba_texture_keeps_channel_order() {
    let Some(context) = gpu_context_or_skip() else { return };
    let texels: Vec<u8> = vec![
        255, 0, 0, 255, /* красный */ 0, 255, 0, 255, /* зелёный */
        0, 0, 255, 255, /* синий */ 255, 255, 255, 128, /* белый, альфа 128 */
    ];
    let texture = SampledTexture::upload(
        &context,
        &TextureDesc {
            extent: vk::Extent2D { width: 2, height: 2 },
            format: TextureFormat::Rgba8Unorm,
            label: "unit-rgba",
        },
        &texels,
    )
    .unwrap();
    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, 2, 2, format);
    let mut pipeline = sample_pipeline(&context, target.format());
    pipeline
        .bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: texture.read_only_binding(),
            }],
        )
        .unwrap();
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
            AttachmentLoad::Clear(BLACK),
        )?;
        pass.draw(&pipeline, 0, &target_size_push(target.extent()), FULLSCREEN)?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    let expected = [RED, GREEN, BLUE, [1.0, 1.0, 1.0, 128.0 / 255.0]];
    for (index, expected_pixel) in expected.iter().enumerate() {
        assert_pixel_near(pixels[index], *expected_pixel, format, &format!("тексель {index}"));
    }
    assert_no_validation_errors(&context);
}

#[test]
fn rendered_target_is_sampled_by_the_next_pass_like_a_blur_pyramid_level() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let format = ColorTargetFormat::Rgba8Unorm;
    // Уровень 0: 8x8. Левый верхний квадрант — вертикальные полосы белый/чёрный шириной 1 px
    // (колонки 0 и 2 белые), остальные квадранты — однотонные цвета.
    let level_zero = color_target(&context, 8, 8, format);
    let level_one = color_target(&context, 4, 4, format);
    let rects = [
        RectInstance::new(0, 0, 1, 4, WHITE),
        RectInstance::new(2, 0, 1, 4, WHITE),
        RectInstance::new(4, 0, 4, 4, RED),
        RectInstance::new(0, 4, 4, 4, GREEN),
        RectInstance::new(4, 4, 4, 4, BLUE),
    ];
    let buffer = rect_buffer(&context, &rects);
    let mut rect_pipe = rect_pipeline(&context, level_zero.format(), BlendMode::Opaque, 1);
    rect_pipe.bind(0, &[instances_write(&buffer)]).unwrap();
    let mut downsample = sample_pipeline(&context, level_one.format());
    downsample
        .bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: DescriptorResource::ReadOnlyImage {
                    view: level_zero.sampled_view().view,
                },
            }],
        )
        .unwrap();
    run_one_shot_blocking(&context, |command_buffer| {
        level_zero.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            level_zero.attachment(),
            AttachmentLoad::Clear(BLACK),
        )?;
        pass.draw(
            &rect_pipe,
            0,
            &target_size_push(level_zero.extent()),
            DrawCall {
                vertices_per_instance: 6,
                instance_count: rects.len() as u32,
                scissor: None,
            },
        )?;
        pass.end();
        level_zero.record_render_to_sampled_read(command_buffer, ConsumerStage::Fragment);

        level_one.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            level_one.attachment(),
            AttachmentLoad::Clear(BLACK),
        )?;
        pass.draw(&downsample, 0, &target_size_push(level_one.extent()), FULLSCREEN)?;
        pass.end();
        Ok(())
    })
    .unwrap();

    // Центр пикселя уровня 1 лежит на стыке 2x2 блока уровня 0: билинейная выборка = среднее блока.
    let pixels = read_color_target_rgba(&level_one, ColorTargetPreviousUse::Rendered).unwrap();
    let gray = [0.5, 0.5, 0.5, 1.0];
    let expected = |x: u32, y: u32| match (x < 2, y < 2) {
        (true, true) => gray,
        (false, true) => RED,
        (true, false) => GREEN,
        (false, false) => BLUE,
    };
    for y in 0..4 {
        for x in 0..4 {
            let actual = pixel_at(&pixels, 4, x, y);
            for channel in 0..4 {
                // 2 LSB: вес билинейной выборки квантуется аппаратным сэмплером.
                assert!(
                    (actual[channel] - expected(x, y)[channel]).abs() <= 2.0 / 255.0,
                    "уровень 1 ({x},{y}) канал {channel}: {actual:?}, ожидали {:?}",
                    expected(x, y)
                );
            }
        }
    }
    // Уровень 0 после чтения сэмплером остаётся читаемым (раскладка восстановлена readback-ом) …
    let source = read_color_target_rgba(
        &level_zero,
        ColorTargetPreviousUse::SampledRead(ConsumerStage::Fragment),
    )
    .unwrap();
    assert_pixel_near(pixel_at(&source, 8, 0, 0), WHITE, format, "полоса 0");
    assert_pixel_near(pixel_at(&source, 8, 1, 0), BLACK, format, "промежуток полос");
    assert_pixel_near(pixel_at(&source, 8, 5, 1), RED, format, "красный квадрант");

    // … и снова пригоден для рисования с сохранением содержимого (запись после чтения, WAR).
    let patch = rect_buffer(&context, &[RectInstance::new(6, 6, 2, 2, WHITE)]);
    rect_pipe.release_slot_after_fence(0).unwrap();
    rect_pipe.bind(0, &[instances_write(&patch)]).unwrap();
    run_one_shot_blocking(&context, |command_buffer| {
        level_zero.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::SampledRead(ConsumerStage::Fragment),
            ContentOnPrepare::Preserve,
        )?;
        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            level_zero.attachment(),
            AttachmentLoad::Preserve,
        )?;
        pass.draw(
            &rect_pipe,
            0,
            &target_size_push(level_zero.extent()),
            DrawCall {
                vertices_per_instance: 6,
                instance_count: 1,
                scissor: None,
            },
        )?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let updated = read_color_target_rgba(&level_zero, ColorTargetPreviousUse::Rendered).unwrap();
    assert_pixel_near(pixel_at(&updated, 8, 7, 7), WHITE, format, "новая заплатка");
    assert_pixel_near(pixel_at(&updated, 8, 5, 5), BLUE, format, "старое содержимое сохранено");
    drop((rect_pipe, downsample, buffer, patch, level_zero, level_one));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
