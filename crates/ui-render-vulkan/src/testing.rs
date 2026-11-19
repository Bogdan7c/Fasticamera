//! Синтетические подложки для тестов и стенда (feature `test-support`): compute-шейдер заполняет
//! storage-картинку узором, из неё публикуется нейтральная [`BackdropSource`] — как это делает ISP.

use ash::vk;
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImagePreviousUse, StorageImageUsage,
};
use gpu_vulkan::swapchain::DisplayColorSpace;

use crate::shaders::blobs;

/// Узор синтетической подложки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackdropPattern {
    /// R = B = (x+0.5)/ширина, G = (y+0.5)/высота: по цвету пикселя читается координата выборки (X дважды — для дисперсии).
    CoordinateGrid,
    /// Сплошной цвет.
    Uniform([f32; 3]),
    /// «Фото»: градиент + шахматка + цветные полосы (детерминированно, для golden).
    Photo,
}

/// Подложка-картинка с узором.
pub struct SyntheticBackdrop {
    image: StorageImage,
    extent: vk::Extent2D,
}

const LOCAL_SIZE: u32 = 8;

impl SyntheticBackdrop {
    /// Создаёт картинку fp16 заданного размера и заполняет её узором (блокирующая разовая отправка).
    pub fn new(context: &GpuContext, extent: vk::Extent2D, pattern: BackdropPattern) -> Result<Self, GpuError> {
        let image = StorageImage::new(
            context,
            &StorageImageDesc {
                extent,
                format_policy: StorageFormatPolicy::HalfFloatOnly,
                usage: StorageImageUsage::ComputeWriteSampledRead,
            },
        )?;
        let mut pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "ui-test-backdrop-fill",
                spirv: blobs::TEST_BACKDROP_FILL_COMP,
                bindings: &[BindingKind::StorageImageWrite],
                push_constant_bytes: 32,
                specialization: &[],
                frame_slots: 1,
            },
        )?;
        pipeline.bind(
            0,
            &[BindingWrite {
                binding: 0,
                resource: image.storage_binding(),
            }],
        )?;
        let (mode, color) = match pattern {
            BackdropPattern::CoordinateGrid => (0_u32, [0.0, 0.0, 0.0, 1.0]),
            BackdropPattern::Uniform([r, g, b]) => (1, [r, g, b, 1.0]),
            BackdropPattern::Photo => (2, [0.0; 4]),
        };
        let mut push: Vec<u8> = [extent.width, extent.height, mode, 0]
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect();
        push.extend(color.iter().flat_map(|value| value.to_ne_bytes()));
        let local = LocalSize::new(LOCAL_SIZE, LOCAL_SIZE)?;
        run_one_shot_blocking(context, |command_buffer| {
            image.record_prepare_for_compute_write(command_buffer, StorageImagePreviousUse::NotUsedYet);
            pipeline.record_dispatch(
                command_buffer,
                0,
                &push,
                WorkgroupCount::covering(extent.width, extent.height, local),
            )?;
            image.record_compute_write_to_sampled_read(command_buffer, ConsumerStage::Fragment);
            Ok(())
        })?;
        Ok(Self { image, extent })
    }

    /// Размер картинки.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Подложка с номером кадра `frame` (картинка та же: меняется только метаданные, как у нового кадра ISP).
    pub fn source(&self, frame: u64) -> BackdropSource {
        // Сабмит записи — заведомо прошлый (разовая отправка ждала завершения).
        BackdropSource::publish(
            &self.image,
            DisplayColorSpace::Srgb,
            BackdropFrameNumber(frame),
            SubmissionSerial(0),
        )
    }
}
