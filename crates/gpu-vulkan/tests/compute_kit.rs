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

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::{read_host_buffer, read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::offscreen::OffscreenTarget;
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageFormat, StorageImagePreviousUse, StorageImageUsage,
};
use gpu_vulkan::swapchain::DisplayColorSpace;
use gpu_vulkan::test_pattern::SurfaceTransform;

/// Режимы узора тестового шейдера (`constant_id = 2`).
const MODE_QUADRANTS: u32 = 0;
const MODE_GRADIENT: u32 = 1;
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const GRADIENT_BLUE: f32 = 0.25;

/// Вариант SPIR-V под выбранный формат (квалификатор формата в шейдере обязан совпасть).
fn pattern_spirv(format: StorageImageFormat) -> &'static [u8] {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => blobs::KIT_TEST_PATTERN_RGB10A2_COMP,
        StorageImageFormat::Rgba16Float => blobs::KIT_TEST_PATTERN_RGBA16F_COMP,
    }
}

/// Допуск сравнения с ожидаемым значением: полшага квантования формата с запасом.
fn tolerance(format: StorageImageFormat) -> f32 {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => 1.0 / 1023.0,
        StorageImageFormat::Rgba16Float => 1.0e-3,
    }
}

fn storage_image(context: &GpuContext, width: u32, height: u32, policy: StorageFormatPolicy) -> StorageImage {
    StorageImage::new(
        context,
        &StorageImageDesc {
            extent: vk::Extent2D { width, height },
            format_policy: policy,
            usage: StorageImageUsage::ComputeWriteSampledRead,
        },
    )
    .unwrap()
}

/// Узор + его ресурсы: пайплайн, uniform цветов четвертей, счётчик пикселей.
struct PatternRig {
    pipeline: ComputePipeline,
    colors: HostBuffer,
    statistics: HostBuffer,
    local: LocalSize,
}

impl PatternRig {
    fn new(context: &GpuContext, format: StorageImageFormat, mode: u32) -> Self {
        let local = LocalSize::new(8, 4).unwrap();
        let pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "kit-test-pattern",
                spirv: pattern_spirv(format),
                bindings: &[
                    BindingKind::StorageImageWrite,
                    BindingKind::UniformBuffer,
                    BindingKind::ReadWriteStorageBuffer,
                ],
                push_constant_bytes: 12,
                specialization: &[
                    SpecializationConstant {
                        constant_id: 0,
                        value: local.x.get(),
                    },
                    SpecializationConstant {
                        constant_id: 1,
                        value: local.y.get(),
                    },
                    SpecializationConstant {
                        constant_id: 2,
                        value: mode,
                    },
                ],
                frame_slots: 2,
            },
        )
        .unwrap();
        let mut colors = HostBuffer::new(context, 64, HostBufferUsage::UniformParameters).unwrap();
        let color_bytes: Vec<u8> = [RED, GREEN, BLUE, WHITE]
            .iter()
            .flatten()
            .flat_map(|channel| channel.to_ne_bytes())
            .collect();
        colors.write_bytes(0, &color_bytes).unwrap();
        let statistics = HostBuffer::new(context, 4, HostBufferUsage::ReadWriteStorage).unwrap();
        Self {
            pipeline,
            colors,
            statistics,
            local,
        }
    }

    fn bind(&mut self, slot: usize, image: &StorageImage) {
        self.pipeline
            .bind(
                slot,
                &[
                    BindingWrite {
                        binding: 0,
                        resource: image.storage_binding(),
                    },
                    BindingWrite {
                        binding: 1,
                        resource: self.colors.binding(),
                    },
                    BindingWrite {
                        binding: 2,
                        resource: self.statistics.binding(),
                    },
                ],
            )
            .unwrap();
    }

    fn push_constants(width: u32, height: u32) -> Vec<u8> {
        let mut bytes: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
        bytes.extend(GRADIENT_BLUE.to_ne_bytes());
        bytes
    }

    /// Записывает полный проход: очистка счётчика, барьер картинки, dispatch, барьеры на чтение.
    fn record(
        &self,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        image: &StorageImage,
        previous: StorageImagePreviousUse,
        reader: ConsumerStage,
    ) -> Result<(), GpuError> {
        let extent = image.extent();
        self.statistics.record_clear_for_compute(command_buffer)?;
        image.record_prepare_for_compute_write(command_buffer, previous);
        self.pipeline.record_dispatch(
            command_buffer,
            slot,
            &Self::push_constants(extent.width, extent.height),
            WorkgroupCount::covering(extent.width, extent.height, self.local),
        )?;
        image.record_compute_write_to_sampled_read(command_buffer, reader);
        self.statistics.record_compute_write_to_host_read(command_buffer);
        Ok(())
    }

    fn written_pixels(&self) -> u32 {
        let bytes = read_host_buffer(&self.statistics).unwrap();
        u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

fn assert_close(actual: [f32; 4], expected: [f32; 4], tolerance: f32, what: &str) {
    for channel in 0..4 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "{what}: канал {channel}: {actual:?} против {expected:?}"
        );
    }
}

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

/// Логический пиксель → физический для поворота дисплея `turns` (обратное к шейдеру компоновщика).
fn logical_to_physical(logical_x: u32, logical_y: u32, turns: u32, width: u32, height: u32) -> (u32, u32) {
    match turns {
        1 => (logical_y, height - 1 - logical_x),
        2 => (width - 1 - logical_x, height - 1 - logical_y),
        3 => (width - 1 - logical_y, logical_x),
        _ => (logical_x, logical_y),
    }
}

#[test]
fn compositor_places_quadrants_for_every_display_rotation_with_black_letterbox() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let source = storage_image(&context, 8, 8, StorageFormatPolicy::PreferTenBitUnorm);
        let mut rig = PatternRig::new(&context, source.format(), MODE_QUADRANTS);
        rig.bind(0, &source);
        run_one_shot_blocking(&context, |command_buffer| {
            rig.record(
                command_buffer,
                0,
                &source,
                StorageImagePreviousUse::NotUsedYet,
                ConsumerStage::Fragment,
            )
        })
        .unwrap();
        let source_before =
            read_storage_image_rgba(&source, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();

        let (physical_width, physical_height) = (24_u32, 16_u32);
        let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 2).unwrap();
        for transform in SurfaceTransform::ROTATIONS {
            let turns = transform.quarter_turns().unwrap();
            let (logical_width, logical_height) = if transform.swaps_extent() {
                (physical_height, physical_width)
            } else {
                (physical_width, physical_height)
            };
            // Полосы по 2 логических пикселя слева и справа.
            let destination = LogicalRect {
                x: 2,
                y: 0,
                width: logical_width - 4,
                height: logical_height,
            };
            let mut target = OffscreenTarget::new(&context, physical_width, physical_height).unwrap();
            target
                .render(|command_buffer, pattern_target| {
                    compositor.record(
                        command_buffer,
                        pattern_target,
                        &CompositeRequest {
                            source: source.sampled_view(),
                            transform,
                            destination,
                            background: BackgroundColor::BLACK,
                            frame_slot: 1,
                        },
                    )
                })
                .unwrap();
            let pixels = target.readback().unwrap();
            let sample = |logical_x: u32, logical_y: u32| {
                let (x, y) = logical_to_physical(logical_x, logical_y, turns, physical_width, physical_height);
                pixels.pixel(x, y).unwrap()
            };
            let right = destination.x + destination.width - 2;
            let bottom = logical_height - 2;
            assert_eq!(sample(3, 1), [255, 0, 0, 255], "{transform:?}: левая верхняя — красная");
            assert_eq!(
                sample(right, 1),
                [0, 255, 0, 255],
                "{transform:?}: правая верхняя — зелёная"
            );
            assert_eq!(
                sample(3, bottom),
                [0, 0, 255, 255],
                "{transform:?}: левая нижняя — синяя"
            );
            assert_eq!(
                sample(right, bottom),
                [255, 255, 255, 255],
                "{transform:?}: правая нижняя — белая"
            );
            for bar_x in [0, 1, logical_width - 2, logical_width - 1] {
                assert_eq!(
                    sample(bar_x, logical_height / 2),
                    [0, 0, 0, 255],
                    "{transform:?}: полоса letterbox x={bar_x} чёрная"
                );
            }
        }
        // Компоновщик только читал источник.
        let source_after =
            read_storage_image_rgba(&source, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();
        assert_eq!(source_before, source_after);
    }
    assert_eq!(context.live_objects(), baseline);
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

/// Набор слота компоновщика нельзя менять, пока он привязан в командном буфере (без UPDATE_AFTER_BIND это
/// делает буфер недействительным). Тот же источник — без обновления, безопасно; другой — `InvalidState`
/// до записи команд; после `release_slot_after_fence` — снова можно.
#[test]
fn compositor_refuses_to_rebind_a_slot_still_bound_in_a_command_buffer() {
    let Some(context) = gpu_context_or_skip() else { return };
    let policy = StorageFormatPolicy::PreferTenBitUnorm;
    let quadrants = storage_image(&context, 8, 8, policy);
    let white = storage_image(&context, 8, 8, policy);
    let mut quadrant_rig = PatternRig::new(&context, quadrants.format(), MODE_QUADRANTS);
    quadrant_rig.bind(0, &quadrants);
    let mut gradient_rig = PatternRig::new(&context, white.format(), MODE_GRADIENT);
    gradient_rig.bind(0, &white);
    run_one_shot_blocking(&context, |command_buffer| {
        let fresh = StorageImagePreviousUse::NotUsedYet;
        quadrant_rig.record(command_buffer, 0, &quadrants, fresh, ConsumerStage::Fragment)?;
        gradient_rig.record(command_buffer, 0, &white, fresh, ConsumerStage::Fragment)
    })
    .unwrap();

    let mut compositor = ImageCompositor::new(&context, OffscreenTarget::FORMAT, 1).unwrap();
    let request = |source: &StorageImage| CompositeRequest {
        source: source.sampled_view(),
        transform: SurfaceTransform::Identity,
        destination: LogicalRect::covering(vk::Extent2D { width: 8, height: 8 }),
        background: BackgroundColor::BLACK,
        frame_slot: 0,
    };
    let mut target = OffscreenTarget::new(&context, 8, 8).unwrap();
    target
        .render(|command_buffer, pattern_target| {
            // Тот же источник дважды в одном буфере — набор не обновляется, это законно.
            compositor.record(command_buffer, pattern_target, &request(&quadrants))?;
            compositor.record(command_buffer, pattern_target, &request(&quadrants))?;
            // Другой источник в тот же слот того же буфера — отказ до записи команд.
            let error = compositor
                .record(command_buffer, pattern_target, &request(&white))
                .unwrap_err();
            assert!(matches!(error, GpuError::InvalidState { .. }), "{error:?}");
            Ok(())
        })
        .unwrap();
    let before_release = target.readback().unwrap();
    assert_eq!(
        before_release.pixel(1, 1).unwrap(),
        [255, 0, 0, 255],
        "нарисован первый источник"
    );

    // Fence слота увиден (render блокирующий): слот можно переписать другим источником.
    compositor.release_slot_after_fence(0).unwrap();
    target
        .render(|command_buffer, pattern_target| compositor.record(command_buffer, pattern_target, &request(&white)))
        .unwrap();
    let after_release = target.readback().unwrap();
    assert_ne!(
        after_release.pixel(1, 1).unwrap(),
        before_release.pixel(1, 1).unwrap(),
        "теперь нарисован второй источник"
    );
    assert!(compositor.release_slot_after_fence(1).is_err(), "слот вне кольца");
    assert_no_validation_errors(&context);
}

#[test]
fn backdrop_consumer_samples_published_image_through_combined_sampler() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (12_u32, 10_u32);
    let image = storage_image(&context, width, height, StorageFormatPolicy::HalfFloatOnly);
    let mut rig = PatternRig::new(&context, image.format(), MODE_GRADIENT);
    rig.bind(0, &image);
    // Сабмит 1 (производитель): запись и перевод на чтение.
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
    let backdrop = BackdropSource::publish(
        &image,
        DisplayColorSpace::Srgb,
        BackdropFrameNumber(1),
        SubmissionSerial(1),
    );
    assert!(backdrop.is_ready_for(SubmissionSerial(2)));

    // Сабмит 2 (потребитель «стекло»): барьер подложки + выборка сэмплером в compute.
    let mut sampler_pipeline = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "kit-test-sample",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let output = HostBuffer::new(
        &context,
        u64::from(width * height) * 16,
        HostBufferUsage::ReadWriteStorage,
    )
    .unwrap();
    sampler_pipeline
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
    let push: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
    run_one_shot_blocking(&context, |command_buffer| {
        backdrop.record_read_barrier(command_buffer, ConsumerStage::Compute);
        sampler_pipeline.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount::covering(width, height, LocalSize::new(8, 8)?),
        )?;
        output.record_compute_write_to_host_read(command_buffer);
        Ok(())
    })
    .unwrap();
    let sampled: Vec<[f32; 4]> = read_host_buffer(&output)
        .unwrap()
        .chunks_exact(16)
        .map(|texel| {
            let channel = |index: usize| {
                f32::from_ne_bytes([
                    texel[index * 4],
                    texel[index * 4 + 1],
                    texel[index * 4 + 2],
                    texel[index * 4 + 3],
                ])
            };
            [channel(0), channel(1), channel(2), channel(3)]
        })
        .collect();
    let direct =
        read_storage_image_rgba(&image, StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment)).unwrap();
    // В центрах пикселей линейная выборка возвращает ровно тексель.
    for (index, (from_sampler, from_readback)) in sampled.iter().zip(&direct).enumerate() {
        assert_close(*from_sampler, *from_readback, 1.0e-3, &format!("тексель {index}"));
    }
    assert_no_validation_errors(&context);
}
