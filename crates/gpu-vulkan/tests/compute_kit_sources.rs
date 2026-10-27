//! Функциональные тесты compute-набора (сессия 07): кадр камеры (`MosaicSource`) как вход compute-шейдера
//! и ошибки набора до работы GPU. Реальный GPU (RADV), полная validation. Узор, компоновщик и подложка —
//! в `compute_kit.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_host_buffer, read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImagePreviousUse, StorageImageUsage,
};

/// Пайплайн «копия RAW в буфер» с привязками (вход, выход) и push-константами `push_words` слов.
fn copy_pipeline(context: &GpuContext, spirv: &[u8], input: BindingKind, push_words: u32) -> ComputePipeline {
    ComputePipeline::new(
        context,
        &ComputePipelineDesc {
            label: "kit-test-raw-copy",
            spirv,
            bindings: &[input, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: push_words * 4,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap()
}

#[test]
fn camera_mosaic_sources_bind_into_compute_and_read_back_exactly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (16_u32, 6_u32);
    let values: Vec<u16> = (0..width * height).map(|index| ((index * 37) % 1024) as u16).collect();

    // RAW16: sampled R16_UINT → texelFetch в compute.
    let raw16 = SyntheticRaw16::new(&context, width, height, &values).unwrap();
    let source = raw16.source();
    let mut pipeline = copy_pipeline(
        &context,
        blobs::KIT_TEST_RAW16_COPY_COMP,
        source.compute_binding_kind(),
        2,
    );
    let output = HostBuffer::new(
        &context,
        u64::from(width * height) * 4,
        HostBufferUsage::ReadWriteStorage,
    )
    .unwrap();
    pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: source.compute_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: output.binding(),
                },
            ],
        )
        .unwrap();
    let push: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
    run_one_shot_blocking(&context, |command_buffer| {
        source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
        pipeline.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount::covering(width, height, LocalSize::new(8, 8)?),
        )?;
        output.record_compute_write_to_host_read(command_buffer);
        source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    let copied: Vec<u16> = read_host_buffer(&output)
        .unwrap()
        .chunks_exact(4)
        .map(|word| u32::from_ne_bytes([word[0], word[1], word[2], word[3]]) as u16)
        .collect();
    assert_eq!(copied, values, "RAW16 прочитан compute-шейдером без искажений");

    // RAW10: упакованные слова как readonly storage-буфер.
    let row_stride_bytes = width * 5 / 4 + 4;
    let packed = pack_raw10(&values, width, row_stride_bytes, 0xA5);
    let raw10 = SyntheticRaw10::new(&context, width, height, row_stride_bytes, &packed).unwrap();
    let source = raw10.source();
    let word_count = (packed.len() / 4) as u32;
    let mut pipeline = copy_pipeline(
        &context,
        blobs::KIT_TEST_RAW10_COPY_COMP,
        source.compute_binding_kind(),
        1,
    );
    let output = HostBuffer::new(&context, u64::from(word_count) * 4, HostBufferUsage::ReadWriteStorage).unwrap();
    pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: source.compute_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: output.binding(),
                },
            ],
        )
        .unwrap();
    run_one_shot_blocking(&context, |command_buffer| {
        source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
        pipeline.record_dispatch(
            command_buffer,
            0,
            &word_count.to_ne_bytes(),
            WorkgroupCount {
                x: word_count.div_ceil(64),
                y: 1,
                z: 1,
            },
        )?;
        output.record_compute_write_to_host_read(command_buffer);
        source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        read_host_buffer(&output).unwrap(),
        packed,
        "RAW10 прочитан словами без искажений"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn kit_errors_are_reported_before_gpu_work_and_leave_no_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let desc =
        |spirv: &'static [u8], push: u32, specialization: &'static [SpecializationConstant]| ComputePipelineDesc {
            label: "kit-test-errors",
            spirv,
            bindings: &[BindingKind::ReadOnlyStorageBuffer, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: push,
            specialization,
            frame_slots: 1,
        };
    assert!(
        ComputePipeline::new(&context, &desc(&[], 4, &[])).is_err(),
        "пустой SPIR-V"
    );
    assert!(
        ComputePipeline::new(&context, &desc(&[1, 2, 3, 4], 4, &[])).is_err(),
        "мусор вместо SPIR-V"
    );
    assert!(
        ComputePipeline::new(&context, &desc(blobs::KIT_TEST_RAW10_COPY_COMP, 3, &[])).is_err(),
        "push не кратен 4"
    );
    const TWICE: [SpecializationConstant; 2] = [
        SpecializationConstant {
            constant_id: 5,
            value: 1,
        },
        SpecializationConstant {
            constant_id: 5,
            value: 2,
        },
    ];
    assert!(
        ComputePipeline::new(&context, &desc(blobs::KIT_TEST_RAW10_COPY_COMP, 4, &TWICE)).is_err(),
        "повтор id"
    );
    assert!(
        StorageImage::new(
            &context,
            &StorageImageDesc {
                extent: vk::Extent2D { width: 0, height: 4 },
                format_policy: StorageFormatPolicy::PreferTenBitUnorm,
                usage: StorageImageUsage::ComputeWriteSampledRead,
            }
        )
        .is_err()
    );
    assert_eq!(
        context.live_objects(),
        baseline,
        "ошибки создания не оставляют объектов"
    );

    let mut pipeline = ComputePipeline::new(&context, &desc(blobs::KIT_TEST_RAW10_COPY_COMP, 4, &[])).unwrap();
    let uniform = HostBuffer::new(&context, 16, HostBufferUsage::UniformParameters).unwrap();
    let storage = HostBuffer::new(&context, 16, HostBufferUsage::ReadWriteStorage).unwrap();
    let wrong_kind = pipeline.bind(
        0,
        &[BindingWrite {
            binding: 0,
            resource: uniform.binding(),
        }],
    );
    assert!(
        matches!(wrong_kind, Err(GpuError::InvalidArgument { .. })),
        "{wrong_kind:?}"
    );
    // Неудачная запись не отметила привязку: dispatch всё ещё запрещён.
    pipeline
        .bind(
            0,
            &[BindingWrite {
                binding: 1,
                resource: storage.binding(),
            }],
        )
        .unwrap();
    let missing = run_one_shot_blocking(&context, |command_buffer| {
        pipeline.record_dispatch(
            command_buffer,
            0,
            &1_u32.to_ne_bytes(),
            WorkgroupCount { x: 1, y: 1, z: 1 },
        )
    })
    .unwrap_err();
    assert!(matches!(missing, GpuError::InvalidState { .. }), "{missing:?}");
    pipeline
        .bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: storage.binding(),
            }],
        )
        .unwrap();
    let wrong_push = run_one_shot_blocking(&context, |command_buffer| {
        pipeline.record_dispatch(command_buffer, 0, &[0; 8], WorkgroupCount { x: 1, y: 1, z: 1 })
    })
    .unwrap_err();
    assert!(matches!(wrong_push, GpuError::InvalidArgument { .. }), "{wrong_push:?}");
    let huge = run_one_shot_blocking(&context, |command_buffer| {
        // Y и Z сразу: гарантированный минимум лимита — 65535, а у RADV лимит X равен u32::MAX
        // (одна ось могла бы пройти проверку и подвесить GPU).
        pipeline.record_dispatch(
            command_buffer,
            0,
            &1_u32.to_ne_bytes(),
            WorkgroupCount {
                x: 1,
                y: u32::MAX,
                z: u32::MAX,
            },
        )
    })
    .unwrap_err();
    assert!(matches!(huge, GpuError::InvalidArgument { .. }), "{huge:?}");
    assert!(pipeline.bind(1, &[]).is_err(), "слот вне кольца");

    let fresh = StorageImage::new(
        &context,
        &StorageImageDesc {
            extent: vk::Extent2D { width: 4, height: 4 },
            format_policy: StorageFormatPolicy::PreferTenBitUnorm,
            usage: StorageImageUsage::ComputeWriteSampledRead,
        },
    )
    .unwrap();
    let unwritten = read_storage_image_rgba(&fresh, StorageImagePreviousUse::NotUsedYet).unwrap_err();
    assert!(matches!(unwritten, GpuError::InvalidState { .. }), "{unwritten:?}");
    drop((pipeline, uniform, storage, fresh));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
