//! Общий стенд тестов compute-набора: узор compute → storage-картинка и проверка цвета (используют
//! `compute_kit.rs` и `compute_kit_compositor.rs`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале; не каждый тестовый бинарь берёт все хелперы"
)]

use ash::vk;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::read_host_buffer;
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageFormat, StorageImagePreviousUse, StorageImageUsage,
};

/// Режимы узора тестового шейдера (`constant_id = 2`).
pub const MODE_QUADRANTS: u32 = 0;
pub const MODE_GRADIENT: u32 = 1;
pub const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
pub const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
pub const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub const GRADIENT_BLUE: f32 = 0.25;

/// Вариант SPIR-V под выбранный формат (квалификатор формата в шейдере обязан совпасть).
pub fn pattern_spirv(format: StorageImageFormat) -> &'static [u8] {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => blobs::KIT_TEST_PATTERN_RGB10A2_COMP,
        StorageImageFormat::Rgba16Float => blobs::KIT_TEST_PATTERN_RGBA16F_COMP,
    }
}

/// Допуск сравнения с ожидаемым значением: полшага квантования формата с запасом.
pub fn tolerance(format: StorageImageFormat) -> f32 {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => 1.0 / 1023.0,
        StorageImageFormat::Rgba16Float => 1.0e-3,
    }
}

pub fn storage_image(context: &GpuContext, width: u32, height: u32, policy: StorageFormatPolicy) -> StorageImage {
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
pub struct PatternRig {
    pub pipeline: ComputePipeline,
    pub colors: HostBuffer,
    pub statistics: HostBuffer,
    pub local: LocalSize,
}

impl PatternRig {
    pub fn new(context: &GpuContext, format: StorageImageFormat, mode: u32) -> Self {
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

    pub fn bind(&mut self, slot: usize, image: &StorageImage) {
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

    pub fn push_constants(width: u32, height: u32) -> Vec<u8> {
        let mut bytes: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
        bytes.extend(GRADIENT_BLUE.to_ne_bytes());
        bytes
    }

    /// Записывает полный проход: очистка счётчика, барьер картинки, dispatch, барьеры на чтение.
    pub fn record(
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

    pub fn written_pixels(&self) -> u32 {
        let bytes = read_host_buffer(&self.statistics).unwrap();
        u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

pub fn assert_close(actual: [f32; 4], expected: [f32; 4], tolerance: f32, what: &str) {
    for channel in 0..4 {
        assert!(
            (actual[channel] - expected[channel]).abs() <= tolerance,
            "{what}: канал {channel}: {actual:?} против {expected:?}"
        );
    }
}
