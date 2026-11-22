//! Цели проявления снимка: картинка RGBA8 и (для JPEG) буфер её копии в память процессора.
//!
//! Это самые крупные выделения снимка: картинка 12 Мп ≈ 48 МБ памяти GPU, копия для JPEG — ещё ≈ 48 МБ,
//! отображённых и обнулённых. На Adreno (14c, release) выделение на потоке рендера стоило 12–17 мс для HEIF и
//! 43–72 мс для JPEG — кадры видоискателя ждали. Поэтому цели выделяются ОТДЕЛЬНО от начала проявления:
//! [`DevelopTargets::allocate`] можно звать на любом потоке (мастерская снимка рендера, решение 144), а
//! [`super::PhotoDeveloper::begin_with_targets`] только проверяет и привязывает готовое.

use std::sync::Arc;

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::host_readback::HostReadbackBuffer;
use gpu_vulkan::rgba8_image::Rgba8StorageImage;

use super::developed::RgbaReadback;
use super::error::PhotoError;

/// Выделенные цели одного проявления.
#[derive(Debug)]
pub struct DevelopTargets {
    pub(crate) image: Arc<Rgba8StorageImage>,
    /// Копия в память процессора: есть ровно тогда, когда её просили (`RgbaReadback::ForJpeg`).
    pub(crate) readback: Option<HostReadbackBuffer>,
}

impl DevelopTargets {
    /// Выделяет цели под снимок размера `image_size` (размер области снимка, `DevelopRequest::crop.size`).
    /// Только память и объекты Vulkan, без команд GPU: годится для любого потока.
    pub fn allocate(context: &GpuContext, image_size: PixelSize, readback: RgbaReadback) -> Result<Self, PhotoError> {
        let image = Arc::new(Rgba8StorageImage::new(
            context,
            vk::Extent2D {
                width: image_size.width,
                height: image_size.height,
            },
        )?);
        let readback = match readback {
            RgbaReadback::NotNeeded => None,
            RgbaReadback::ForJpeg => Some(HostReadbackBuffer::new(context, image.packed_size_bytes())?),
        };
        Ok(Self { image, readback })
    }

    /// Подходят ли цели запросу: размер картинки и наличие копии совпадают с тем, что нужно проявлению.
    pub(crate) fn mismatch_with(&self, image_size: PixelSize, readback: RgbaReadback) -> Option<String> {
        let extent = self.image.extent();
        if (extent.width, extent.height) != (image_size.width, image_size.height) {
            return Some(format!(
                "картинка целей {}x{}, а снимку нужна {}x{}",
                extent.width, extent.height, image_size.width, image_size.height
            ));
        }
        let wants_readback = matches!(readback, RgbaReadback::ForJpeg);
        if wants_readback != self.readback.is_some() {
            return Some(format!(
                "копия в память: нужна {wants_readback}, в целях {}",
                self.readback.is_some()
            ));
        }
        None
    }
}
