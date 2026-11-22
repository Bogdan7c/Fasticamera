//! Загрузка текстуры, записанная в командный буфер кадра (сессия 14g): без блокирующей отправки,
//! в одном буфере с чтением — как кадр UI, который загружает картинку и сразу рисует её.
//!
//! Доказывается: данные доходят до шейдера в верном порядке строк и каналов; проверки staging идут ДО
//! первой команды (при отказе буфер пригоден, объекты не утекли); брошенная без записи текстура
//! уничтожается.

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
use gpu_vulkan::compute::BindingWrite;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, DrawCall, RasterPass};
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use raster_support::{BLACK, assert_pixel_near, color_target, pixel_at, sample_pipeline, target_size_push};

const FULLSCREEN: DrawCall = DrawCall {
    vertices_per_instance: 3,
    instance_count: 1,
    scissor: None,
};

fn rgba_desc(width: u32, height: u32) -> TextureDesc {
    TextureDesc {
        extent: vk::Extent2D { width, height },
        format: TextureFormat::Rgba8Unorm,
        label: "unit-recorded",
    }
}

#[test]
fn recorded_upload_reaches_the_shader_in_the_same_command_buffer_without_blocking() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    // Неквадратная 3x2 с различимыми пикселями: перепутанные строки или каналы дадут другой результат.
    let (width, height) = (3_u32, 2_u32);
    let texels: Vec<u8> = (0..width * height)
        .flat_map(|index| {
            [
                (index * 40 + 10) as u8,
                (255 - index * 30) as u8,
                (index * 7) as u8,
                255,
            ]
        })
        .collect();
    let desc = rgba_desc(width, height);
    assert_eq!(desc.texel_bytes(), 24);
    let mut staging = HostBuffer::new(&context, desc.texel_bytes(), HostBufferUsage::ReadOnlyStorage).unwrap();
    staging.write_bytes(0, &texels).unwrap();
    let pending = SampledTexture::allocate_for_recorded_upload(&context, &desc).unwrap();

    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, width, height, format);
    let mut pipeline = sample_pipeline(&context, target.format());
    let mut texture = None;
    run_one_shot_blocking(&context, |command_buffer| {
        // Загрузка и чтение — в ОДНОМ буфере: барьер после копии обязан успеть к фрагментному шейдеру.
        let uploaded = pending.record_upload_from(command_buffer, &staging)?;
        pipeline.bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: uploaded.read_only_binding(),
            }],
        )?;
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
        texture = Some(uploaded);
        Ok(())
    })
    .unwrap();
    let texture = texture.unwrap();
    assert_eq!(texture.extent(), desc.extent);

    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for y in 0..height {
        for x in 0..width {
            let base = ((y * width + x) * 4) as usize;
            let expected = [
                f32::from(texels[base]) / 255.0,
                f32::from(texels[base + 1]) / 255.0,
                f32::from(texels[base + 2]) / 255.0,
                1.0,
            ];
            assert_pixel_near(pixel_at(&pixels, width, x, y), expected, format, &format!("({x},{y})"));
        }
    }
    drop((pipeline, target, texture, staging));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn staging_that_cannot_feed_the_copy_is_rejected_before_any_command() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let desc = rgba_desc(4, 4);

    // Слишком маленький staging и staging не того назначения: оба отказа — InvalidArgument.
    let small = HostBuffer::new(&context, desc.texel_bytes() - 1, HostBufferUsage::ReadOnlyStorage).unwrap();
    let wrong_usage = HostBuffer::new(&context, desc.texel_bytes(), HostBufferUsage::ReadWriteStorage).unwrap();
    for (staging, what) in [(&small, "маленький"), (&wrong_usage, "не тот usage")] {
        let pending = SampledTexture::allocate_for_recorded_upload(&context, &desc).unwrap();
        let mut outcome = None;
        run_one_shot_blocking(&context, |command_buffer| {
            outcome = Some(pending.record_upload_from(command_buffer, staging));
            // Отказ до первой команды: буфер остаётся пустым и пригодным к отправке (validation молчит).
            Ok(())
        })
        .unwrap();
        assert!(
            matches!(outcome, Some(Err(GpuError::InvalidArgument { .. }))),
            "{what}: {outcome:?}"
        );
    }
    drop((small, wrong_usage));
    assert_eq!(context.live_objects(), baseline, "отказы не оставили объектов");
    assert_no_validation_errors(&context);
}

#[test]
fn allocation_checks_the_description_before_creating_objects_and_unused_pending_is_destroyed() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let empty = rgba_desc(0, 4);
    assert!(matches!(
        SampledTexture::allocate_for_recorded_upload(&context, &empty),
        Err(GpuError::InvalidArgument { .. })
    ));
    let too_wide = rgba_desc(1_000_000, 1);
    assert!(matches!(
        SampledTexture::allocate_for_recorded_upload(&context, &too_wide),
        Err(GpuError::InvalidArgument { .. })
    ));
    assert_eq!(context.live_objects(), baseline, "отказы не создали объектов");

    let pending = SampledTexture::allocate_for_recorded_upload(&context, &rgba_desc(2, 2)).unwrap();
    assert!(
        context.live_objects().total() > baseline.total(),
        "выделенная текстура видна в учёте объектов"
    );
    drop(pending);
    assert_eq!(
        context.live_objects(),
        baseline,
        "брошенная без записи текстура уничтожена"
    );
    assert_no_validation_errors(&context);
}
