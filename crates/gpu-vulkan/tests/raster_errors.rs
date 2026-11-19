//! Тесты отказов растрового набора (сессия 11): каждая ошибка — свой вариант `GpuError`, и она
//! возникает ДО записи команд / создания объектов Vulkan. Доказательство «до записи»: после ошибок
//! тот же проход остаётся открытым, допускает корректный `draw`, а на readback виден только он.

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
use gpu_vulkan::compute::{BindingKind, BindingWrite, DescriptorResource};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::{read_color_target_rgba, run_one_shot_blocking};
use gpu_vulkan::raster::{AttachmentLoad, BlendMode, DrawCall, RasterPass, RasterPipeline, RasterPipelineDesc};
use gpu_vulkan::sampled_texture::{SampledTexture, TextureDesc, TextureFormat};
use gpu_vulkan::shaders::blobs;
use raster_support::{
    BLACK, GREEN, RED, RectInstance, assert_pixel_near, color_target, instances_write, pixel_at, rect_buffer,
    rect_pipeline, target_size_push,
};

const RECT_DRAW: DrawCall = DrawCall {
    vertices_per_instance: 6,
    instance_count: 1,
    scissor: None,
};

#[test]
fn rejected_draws_record_nothing_and_leave_the_pass_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, 8, 8, format);
    let other_format_target = color_target(&context, 8, 8, ColorTargetFormat::Rgba16Float);
    let buffer = rect_buffer(&context, &[RectInstance::new(1, 1, 4, 4, GREEN)]);
    let mut pipeline = rect_pipeline(&context, target.format(), BlendMode::Opaque, 2);
    pipeline.bind(0, &[instances_write(&buffer)]).unwrap();
    // Слот 1 намеренно НЕ записан.
    let push = target_size_push(target.extent());

    run_one_shot_blocking(&context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        other_format_target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;

        // Формат конвейера ≠ формату цели.
        let mut wrong_pass = RasterPass::begin(
            &context,
            command_buffer,
            other_format_target.attachment(),
            AttachmentLoad::Clear(BLACK),
        )?;
        let error = wrong_pass.draw(&pipeline, 0, &push, RECT_DRAW).unwrap_err();
        assert!(
            matches!(&error, GpuError::InvalidArgument { what } if what.contains("формат")),
            "{error:?}"
        );
        wrong_pass.end();

        let mut pass = RasterPass::begin(
            &context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(BLACK),
        )?;
        // Длина push-констант.
        let error = pass.draw(&pipeline, 0, &push[..4], RECT_DRAW).unwrap_err();
        assert!(
            matches!(&error, GpuError::InvalidArgument { what } if what.contains("push")),
            "{error:?}"
        );
        // Привязки слота 1 не записаны.
        let error = pass.draw(&pipeline, 1, &push, RECT_DRAW).unwrap_err();
        assert!(
            matches!(&error, GpuError::InvalidState { what } if what.contains("не записаны")),
            "{error:?}"
        );
        // Слот вне кольца.
        let error = pass.draw(&pipeline, 7, &push, RECT_DRAW).unwrap_err();
        assert!(
            matches!(&error, GpuError::InvalidArgument { what } if what.contains("слот")),
            "{error:?}"
        );
        // Scissor за целью (и со сдвигом в минус).
        for bad in [
            vk::Rect2D {
                offset: vk::Offset2D { x: 4, y: 4 },
                extent: vk::Extent2D { width: 5, height: 2 },
            },
            vk::Rect2D {
                offset: vk::Offset2D { x: -1, y: 0 },
                extent: vk::Extent2D { width: 2, height: 2 },
            },
        ] {
            let error = pass
                .draw(
                    &pipeline,
                    0,
                    &push,
                    DrawCall {
                        scissor: Some(bad),
                        ..RECT_DRAW
                    },
                )
                .unwrap_err();
            assert!(
                matches!(&error, GpuError::InvalidArgument { what } if what.contains("scissor")),
                "{error:?}"
            );
        }
        // Пустой вызов после успешных проверок — допустимая пустая операция.
        pass.draw(
            &pipeline,
            0,
            &push,
            DrawCall {
                instance_count: 0,
                ..RECT_DRAW
            },
        )?;
        // Корректный draw после всех отказов работает.
        pass.draw(&pipeline, 0, &push, RECT_DRAW)?;
        pass.end();
        Ok(())
    })
    .unwrap();

    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            let inside = (1..5).contains(&x) && (1..5).contains(&y);
            assert_pixel_near(
                pixel_at(&pixels, 8, x, y),
                if inside { GREEN } else { BLACK },
                format,
                &format!("({x},{y})"),
            );
        }
    }
    drop((pipeline, buffer, target, other_format_target));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn pipeline_without_bindings_draws_with_push_constants_only() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, 4, 4, format);
    let mut pipeline = RasterPipeline::new(
        &context,
        &RasterPipelineDesc {
            label: "kit-raster-solid",
            vertex_spirv: blobs::FULLSCREEN_TRIANGLE_VERT,
            fragment_spirv: blobs::KIT_RASTER_SOLID_FRAG,
            bindings: &[],
            push_constant_bytes: 16,
            specialization: &[],
            color_format: target.format(),
            blend: BlendMode::Opaque,
            frame_slots: 2,
        },
    )
    .unwrap();
    assert!(pipeline.bindings().is_empty());
    assert_eq!(pipeline.frame_slots(), 2);
    assert_eq!(pipeline.color_format(), target.format());
    // Нет привязок: пустая запись допустима, непустая — ошибка, слот вне кольца — ошибка.
    pipeline.bind(0, &[]).unwrap();
    let write = BindingWrite {
        binding: 0,
        resource: DescriptorResource::ReadOnlyImage {
            view: vk::ImageView::null(),
        },
    };
    assert!(matches!(
        pipeline.bind(0, &[write]),
        Err(GpuError::InvalidArgument { .. })
    ));
    assert!(matches!(pipeline.bind(2, &[]), Err(GpuError::InvalidArgument { .. })));
    pipeline.release_slot_after_fence(1).unwrap();
    assert!(matches!(
        pipeline.release_slot_after_fence(2),
        Err(GpuError::InvalidArgument { .. })
    ));

    let color = [0.0_f32, 0.5, 1.0, 1.0];
    let push: Vec<u8> = color.iter().flat_map(|value| value.to_ne_bytes()).collect();
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
            AttachmentLoad::Clear(RED),
        )?;
        pass.draw(
            &pipeline,
            0,
            &push,
            DrawCall {
                vertices_per_instance: 3,
                instance_count: 1,
                scissor: None,
            },
        )?;
        pass.end();
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for (index, pixel) in pixels.iter().enumerate() {
        assert_pixel_near(*pixel, color, format, &format!("пиксель {index}"));
    }
    drop((pipeline, target));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn dropping_an_unfinished_pass_closes_rendering_so_the_command_buffer_stays_valid() {
    let Some(context) = gpu_context_or_skip() else { return };
    let format = ColorTargetFormat::Rgba8Unorm;
    let target = color_target(&context, 4, 4, format);
    run_one_shot_blocking(&context, |command_buffer| {
        target.record_prepare_for_render(
            command_buffer,
            ColorTargetPreviousUse::NotUsedYet,
            ContentOnPrepare::Discard,
        )?;
        // Проход брошен без end(): Drop закрывает рендеринг. Иначе vkEndCommandBuffer/сабмит упали бы
        // или validation сообщила бы о незакрытом рендеринге.
        drop(RasterPass::begin(
            &context,
            command_buffer,
            target.attachment(),
            AttachmentLoad::Clear(RED),
        )?);
        Ok(())
    })
    .unwrap();
    let pixels = read_color_target_rgba(&target, ColorTargetPreviousUse::Rendered).unwrap();
    for pixel in &pixels {
        assert_pixel_near(*pixel, RED, format, "очистка из брошенного прохода");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_descriptions_and_uploads_fail_before_creating_any_object() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let good = RasterPipelineDesc {
        label: "bad-desc",
        vertex_spirv: blobs::KIT_RASTER_RECTS_VERT,
        fragment_spirv: blobs::KIT_RASTER_RECTS_FRAG,
        bindings: &[BindingKind::ReadOnlyStorageBuffer],
        push_constant_bytes: 8,
        specialization: &[],
        color_format: vk::Format::R8G8B8A8_UNORM,
        blend: BlendMode::Opaque,
        frame_slots: 1,
    };
    let bad_variants = [
        RasterPipelineDesc {
            vertex_spirv: &[],
            ..good
        },
        RasterPipelineDesc {
            fragment_spirv: &[],
            ..good
        },
        RasterPipelineDesc {
            push_constant_bytes: 6,
            ..good
        },
        RasterPipelineDesc {
            push_constant_bytes: 1 << 20,
            ..good
        },
        RasterPipelineDesc { frame_slots: 0, ..good },
    ];
    for desc in &bad_variants {
        let error = RasterPipeline::new(&context, desc).unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
    }
    // Формат без возможности рисования (сжатый/глубины) — Unsupported, а не InvalidArgument.
    let unsupported = RasterPipelineDesc {
        color_format: vk::Format::D32_SFLOAT,
        ..good
    };
    assert!(matches!(
        RasterPipeline::new(&context, &unsupported),
        Err(GpuError::Unsupported { .. })
    ));
    assert_eq!(context.live_objects(), baseline, "отказы не создали объектов");

    // Загрузка текстуры: неверная длина данных и нулевой размер.
    let desc = TextureDesc {
        extent: vk::Extent2D { width: 4, height: 4 },
        format: TextureFormat::R8Unorm,
        label: "bad-upload",
    };
    for texels in [&[0_u8; 15][..], &[0_u8; 17][..], &[][..]] {
        let error = SampledTexture::upload(&context, &desc, texels).unwrap_err();
        assert!(
            matches!(&error, GpuError::InvalidArgument { what } if what.contains("данных")),
            "{error:?}"
        );
    }
    let empty = TextureDesc {
        extent: vk::Extent2D { width: 0, height: 4 },
        ..desc
    };
    assert!(matches!(
        SampledTexture::upload(&context, &empty, &[]),
        Err(GpuError::InvalidArgument { .. })
    ));
    assert_eq!(context.live_objects(), baseline, "отказы загрузки не создали объектов");
    assert_no_validation_errors(&context);
}

#[test]
fn preserving_a_target_that_was_never_rendered_is_rejected_without_recording() {
    let Some(context) = gpu_context_or_skip() else { return };
    let target = color_target(&context, 4, 4, ColorTargetFormat::Rgba8Unorm);
    run_one_shot_blocking(&context, |command_buffer| {
        let error = target
            .record_prepare_for_render(
                command_buffer,
                ColorTargetPreviousUse::NotUsedYet,
                ContentOnPrepare::Preserve,
            )
            .unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        read_color_target_rgba(&target, ColorTargetPreviousUse::NotUsedYet),
        Err(GpuError::InvalidState { .. })
    ));
    assert_no_validation_errors(&context);
}
