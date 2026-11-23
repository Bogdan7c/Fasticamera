//! Синтетическая ЖИВАЯ картинка для тестов (feature `test-support`): compute-шейдер заполняет storage-картинку
//! `Rgba16Float` узором с прямой альфой и отдаёт её так, как это делает производитель настоящих живых картинок
//! (приборы цвета 14h): запись compute → барьер на чтение во фрагментном шейдере → [`LiveImageInput`].

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImagePreviousUse, StorageImageUsage,
};

use crate::live_images::LiveImageInput;
use crate::shaders::blobs;

const LOCAL_SIZE: u32 = 8;

/// Пиксель узора (формулы те же, что в `test_live_fill.comp`): цвет экрана, прямая альфа.
pub fn pattern_texel(x: u32, y: u32, width: u32, height: u32) -> [f32; 4] {
    [
        x as f32 / width as f32,
        y as f32 / height as f32,
        if (x + y).is_multiple_of(2) { 0.75 } else { 0.25 },
        1.0 - x as f32 / (2.0 * width as f32),
    ]
}

/// Живая картинка с узором в видеопамяти.
pub struct SyntheticLiveImage {
    image: StorageImage,
    extent: vk::Extent2D,
}

impl SyntheticLiveImage {
    /// Создаёт картинку и заполняет её узором (блокирующая разовая отправка). После вызова картинка в
    /// раскладке для чтения фрагментным шейдером — договор производителя выполнен.
    pub fn new(context: &GpuContext, extent: vk::Extent2D) -> Result<Self, GpuError> {
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
                label: "ui-test-live-fill",
                spirv: blobs::TEST_LIVE_FILL_COMP,
                bindings: &[BindingKind::StorageImageWrite],
                push_constant_bytes: 8,
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
        let push: Vec<u8> = [extent.width, extent.height]
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect();
        let local = LocalSize::new(LOCAL_SIZE, LOCAL_SIZE)?;
        run_one_shot_blocking(context, |command_buffer| {
            image.record_prepare_for_compute_write(command_buffer, StorageImagePreviousUse::NotUsedYet);
            pipeline.record_dispatch(
                command_buffer,
                0,
                &push,
                WorkgroupCount::covering(extent.width, extent.height, local),
            )?;
            image.record_compute_write_to_fragment_read(command_buffer);
            Ok(())
        })?;
        Ok(Self { image, extent })
    }

    /// Вход для таблицы живых картинок кадра.
    pub fn input(&self) -> LiveImageInput {
        LiveImageInput {
            view: self.image.sampled_view(),
        }
    }

    /// Пиксели картинки на стороне CPU (построчно) — то, что должно лежать в видеопамяти.
    pub fn texels(&self) -> Vec<[f32; 4]> {
        let (width, height) = (self.extent.width, self.extent.height);
        (0..height)
            .flat_map(|y| (0..width).map(move |x| pattern_texel(x, y, width, height)))
            .collect()
    }

    /// Размер картинки.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }
}
