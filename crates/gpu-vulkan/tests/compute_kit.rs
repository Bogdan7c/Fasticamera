//! Функциональные тесты публичного compute-набора (сессия 07) на реальном GPU (RADV, полная validation):
//! узор compute → storage-картинка, компоновщик, подложка. Кадр камеры и ошибки — `compute_kit_sources.rs`.
//!
//! Каждый тест проверяет наблюдаемый результат: пиксели после readback, значения буферов, метки
//! времени, счётчики живых объектов. Сломанный код (не та привязка, не тот барьер, не тот поворот,
//! не применённая specialization-константа) даёт другие числа, и тест падает.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;
mod compute_kit_rig;

use common::{assert_no_validation_errors, gpu_context_or_skip};
use compute_kit_rig::{
    BLUE, GRADIENT_BLUE, GREEN, MODE_GRADIENT, MODE_QUADRANTS, PatternRig, RED, WHITE, assert_close, storage_image,
    tolerance,
};
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{StorageFormatPolicy, StorageImagePreviousUse};

#[test]
fn compute_gradient_reaches_storage_image_with_all_kit_parts_and_timestamps() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    for policy in [
        StorageFormatPolicy::PreferTenBitUnorm,
        StorageFormatPolicy::HalfFloatOnly,
    ] {
        let (width, height) = (96_u32, 64_u32);
        let image = storage_image(&context, width, height, policy);
        let format = image.format();
        let mut rig = PatternRig::new(&context, format, MODE_GRADIENT);
        // Пишем через слот 1: слот 0 намеренно не трогаем.
        rig.bind(1, &image);
        let mut timestamps = GpuTimestamps::new(&context, 1, &["до прохода", "после прохода"]).unwrap();
        run_one_shot_blocking(&context, |command_buffer| {
            timestamps.record_reset(command_buffer, 0)?;
            timestamps.record_point(command_buffer, 0, 0, TimestampStage::TopOfPipe)?;
            rig.record(
                command_buffer,
                1,
                &image,
                StorageImagePreviousUse::NotUsedYet,
                ConsumerStage::Compute,
            )?;
            timestamps.record_point(command_buffer, 0, 1, TimestampStage::ComputeShaderDone)
        })
        .unwrap();

        let texels =
            read_storage_image_rgba(&image, StorageImagePreviousUse::SampledRead(ConsumerStage::Compute)).unwrap();
        assert_eq!(texels.len(), (width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let expected = [
                    x as f32 / (width - 1) as f32,
                    y as f32 / (height - 1) as f32,
                    GRADIENT_BLUE,
                    1.0,
                ];
                let actual = texels[(y * width + x) as usize];
                assert_close(actual, expected, tolerance(format), &format!("{format:?} ({x}, {y})"));
            }
        }
        assert_eq!(
            rig.written_pixels(),
            width * height,
            "каждая нить внутри картинки отметилась ровно раз"
        );
        // Слот 0 ни разу не записан: dispatch через него — явная ошибка, а не мусорный дескриптор.
        let unbound = run_one_shot_blocking(&context, |command_buffer| {
            rig.pipeline.record_dispatch(
                command_buffer,
                0,
                &PatternRig::push_constants(1, 1),
                WorkgroupCount { x: 1, y: 1, z: 1 },
            )
        })
        .unwrap_err();
        assert!(matches!(unbound, GpuError::InvalidState { .. }), "{unbound:?}");

        let TimestampCollect::Ready(spans) = timestamps.try_collect(0).unwrap() else {
            panic!("метки готовы после разовой отправки");
        };
        assert_eq!(spans.spans.len(), 1);
        assert!(spans.total_ns > 0, "проход занял положительное время: {spans:?}");
    }
    assert_eq!(context.live_objects(), baseline, "всё освобождено");
    assert_no_validation_errors(&context);
}

#[test]
fn specialization_constant_switches_pattern_to_quadrants() {
    let Some(context) = gpu_context_or_skip() else { return };
    let image = storage_image(&context, 16, 8, StorageFormatPolicy::PreferTenBitUnorm);
    let mut rig = PatternRig::new(&context, image.format(), MODE_QUADRANTS);
    rig.bind(0, &image);
    run_one_shot_blocking(&context, |command_buffer| {
        rig.record(
            command_buffer,
            0,
            &image,
            StorageImagePreviousUse::NotUsedYet,
            ConsumerStage::Fragment,
        )
    })
    .unwrap();
    let texels =
        read_storage_image_rgba(&image, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();
    let at = |x: u32, y: u32| texels[(y * 16 + x) as usize];
    assert_eq!(at(0, 0), RED);
    assert_eq!(at(15, 0), GREEN);
    assert_eq!(at(0, 7), BLUE);
    assert_eq!(at(15, 7), WHITE);
    assert_eq!(at(7, 3), RED, "граница четвертей — по середине");
    assert_eq!(at(8, 4), WHITE);
    assert_no_validation_errors(&context);
}

/// `check_dispatch` даёт те же отказы, что `record_dispatch`, но без командного буфера — чтобы
/// производитель проверил весь кадр до первой команды; устройство отвергает недопустимую рабочую группу.
#[test]
fn dispatch_and_workgroup_checks_reject_bad_input_without_recording() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut pipeline = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "kit-test-check",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let one_group = WorkgroupCount { x: 1, y: 1, z: 1 };
    assert!(
        matches!(
            pipeline.check_dispatch(0, 8, one_group),
            Err(GpuError::InvalidState { .. })
        ),
        "привязки ещё не записаны"
    );
    assert!(
        pipeline.check_dispatch_shape(8, one_group).is_ok(),
        "форма dispatch верна"
    );
    let image = storage_image(&context, 4, 4, StorageFormatPolicy::HalfFloatOnly);
    let output = HostBuffer::new(&context, 256, HostBufferUsage::ReadWriteStorage).unwrap();
    pipeline
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: image.read_only_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: output.binding(),
                },
            ],
        )
        .unwrap();
    assert!(pipeline.check_dispatch(0, 8, one_group).is_ok());
    assert!(
        pipeline.check_dispatch(0, 4, one_group).is_err(),
        "push-константы не того размера"
    );
    assert!(pipeline.check_dispatch(1, 8, one_group).is_err(), "слот вне кольца");
    // Ось Y: RADV разрешает по X все 2^32-1 групп, а по Y — 65535 (минимум спецификации тоже 65535).
    let beyond_device = WorkgroupCount {
        x: 1,
        y: u32::MAX,
        z: 1,
    };
    assert!(
        pipeline.check_dispatch(0, 8, beyond_device).is_err(),
        "групп больше лимита устройства"
    );
    // Любое устройство: ≤ maxComputeWorkGroupInvocations нитей (спецификация гарантирует ≥ 128), но не 2^20.
    assert!(LocalSize::new(8, 8).unwrap().validate_for_device(&context).is_ok());
    assert!(
        LocalSize::new(1024, 1024)
            .unwrap()
            .validate_for_device(&context)
            .is_err()
    );
    assert_no_validation_errors(&context);
}
