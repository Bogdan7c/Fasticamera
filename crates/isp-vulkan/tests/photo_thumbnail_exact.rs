//! Шейдер миниатюры снимка БИТ-В-БИТ равен прежнему (сессия 16a, решение 213).
//!
//! Эталон — замороженная копия прежнего шейдера `photo_thumbnail_reference.comp` (до оптимизаций сессии 16a).
//! Тест заполняет картинку детерминированными кодами (`test_photo_thumbnail_fill.comp`: каждый канал проходит ВСЕ
//! 256 значений), гонит по ОДНОЙ и той же картинке прежний и продуктовый шейдеры миниатюры и сравнивает ВСЕ байты
//! обеих миниатюр. Матрица: поворот × размер миниатюры × размер картинки × заполнение.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod quality_support;

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use isp_vulkan::photo::PhotoConfig;
use isp_vulkan::shaders::blobs;
use quality_support::{assert_no_validation_errors, gpu_context_or_skip};
use still_contract::shot::StillRotation;

/// Группа миниатюры (как в продукте: фиксированная).
const GROUP: u32 = 16;

fn pipeline(context: &GpuContext, label: &'static str, spirv: &[u8]) -> ComputePipeline {
    ComputePipeline::new(
        context,
        &ComputePipelineDesc {
            label,
            spirv,
            bindings: &[BindingKind::StorageImageWrite, BindingKind::StorageImageWrite],
            push_constant_bytes: 32,
            specialization: &[
                SpecializationConstant {
                    constant_id: 0,
                    value: GROUP,
                },
                SpecializationConstant {
                    constant_id: 1,
                    value: GROUP,
                },
            ],
            frame_slots: 1,
        },
    )
    .unwrap()
}

/// Миниатюра `developed` выбранным шейдером: байты RGBA8.
fn thumbnail_of(
    context: &GpuContext,
    pipeline: &mut ComputePipeline,
    developed: &Rgba8StorageImage,
    size: PixelSize,
    rotation: u32,
) -> Vec<u8> {
    let target = Rgba8StorageImage::new(
        context,
        vk::Extent2D {
            width: size.width,
            height: size.height,
        },
    )
    .unwrap();
    pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: developed.storage_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: target.storage_binding(),
                },
            ],
        )
        .unwrap();
    // Раскладка push-констант `PlaneGeometry`: ширина и высота в нитях, cfa, режим (поворот), vec4.
    let words = [size.width * GROUP, size.height * GROUP, 0, rotation];
    let push: Vec<u8> = words
        .iter()
        .flat_map(|word| word.to_ne_bytes())
        .chain([0_u8; 16])
        .collect();
    run_one_shot_blocking(context, |command_buffer| {
        target.record_prepare_for_compute_write(command_buffer);
        pipeline.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount {
                x: size.width,
                y: size.height,
                z: 1,
            },
        )?;
        // Миниатюра готова к чтению на CPU (`GENERAL → TRANSFER_SRC`).
        target.record_compute_write_to_transfer_read(command_buffer);
        Ok(())
    })
    .unwrap();
    pipeline.release_slot_after_fence(0).unwrap();
    target.read_back_blocking().unwrap()
}

/// Картинка `width`×`height`, заполненная кодами по `values` (см. шейдер заполнения).
fn filled_image(
    context: &GpuContext,
    fill: &mut ComputePipeline,
    width: u32,
    height: u32,
    values: [u32; 3],
) -> Rgba8StorageImage {
    let image = Rgba8StorageImage::new(context, vk::Extent2D { width, height }).unwrap();
    fill.bind(
        0,
        &[BindingWrite {
            binding: 0,
            resource: image.storage_binding(),
        }],
    )
    .unwrap();
    let push: Vec<u8> = values
        .iter()
        .chain(&[0_u32])
        .flat_map(|word| word.to_ne_bytes())
        .collect();
    run_one_shot_blocking(context, |command_buffer| {
        image.record_prepare_for_compute_write(command_buffer);
        fill.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount {
                x: width.div_ceil(8),
                y: height.div_ceil(8),
                z: 1,
            },
        )?;
        // Записанное заполнением читает шейдер миниатюры (следующий сабмит той же очереди).
        gpu_vulkan::device_buffer::record_compute_pass_dependency(context, command_buffer);
        Ok(())
    })
    .unwrap();
    fill.release_slot_after_fence(0).unwrap();
    image
}

#[test]
fn production_thumbnail_shader_is_bit_identical_to_the_frozen_one() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut frozen = pipeline(&context, "reference-thumbnail", blobs::PHOTO_THUMBNAIL_REFERENCE_COMP);
    let mut production = pipeline(&context, "production-thumbnail", blobs::PHOTO_THUMBNAIL_COMP);
    let mut fill = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "test-thumbnail-fill",
            spirv: blobs::TEST_PHOTO_THUMBNAIL_FILL_COMP,
            bindings: &[BindingKind::StorageImageWrite],
            push_constant_bytes: 16,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let rotations = [
        (StillRotation::None, 0),
        (StillRotation::Clockwise90, 1),
        (StillRotation::Clockwise180, 2),
        (StillRotation::Clockwise270, 3),
    ];
    // Картинки: чётная, нечётная, узкая; разные множители и сдвиги заполнения.
    for (width, height, values) in [
        (128_u32, 96_u32, [5_u32, 3, 0]),
        (101, 67, [7, 13, 41]),
        (33, 130, [11, 5, 200]),
    ] {
        let source = filled_image(&context, &mut fill, width, height, values);
        for (rotation, code) in rotations {
            for long_side in [7_u32, 16, 40] {
                let config = PhotoConfig {
                    thumbnail_long_side_px: long_side,
                    ..PhotoConfig::default()
                };
                let size = config
                    .thumbnail_plan(PixelSize::new(width, height), rotation)
                    .unwrap()
                    .size();
                let old = thumbnail_of(&context, &mut frozen, &source, size, code);
                let new = thumbnail_of(&context, &mut production, &source, size, code);
                assert_eq!(old.len(), (size.width * size.height * 4) as usize);
                assert!(
                    old.iter().any(|&byte| byte != 0) && old.iter().any(|&byte| byte != old[0]),
                    "миниатюра вырождена: сравнение ничего не доказывает"
                );
                assert!(
                    old == new,
                    "{width}x{height} / {rotation:?} / {size:?}: миниатюра отличается от прежней"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}
