//! ЧИСТЫЙ выбор параметров swapchain цели кодера по возможностям surface (без вызовов Vulkan: тестируется
//! на ПК с синтетическими возможностями).

use ash::vk;

use super::EncoderTargetError;
use crate::swapchain::SurfaceSupport;

/// 8-бит UNORM-форматы по порядку предпочтения (RGBA совпадает с `Rgba8StorageImage` — blit без перестановки).
const ENCODER_FORMATS: [vk::Format; 2] = [vk::Format::R8G8B8A8_UNORM, vk::Format::B8G8R8A8_UNORM];

/// Итог выбора для создания swapchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSwapchainPlan {
    /// Формат и цветовое пространство (всегда `SRGB_NONLINEAR`).
    pub surface_format: vk::SurfaceFormatKHR,
    /// `minImageCount` для создания.
    pub image_count: u32,
    /// Размер картинок = размер снимка.
    pub extent: vk::Extent2D,
    /// `preTransform = currentTransform` (поворот не применяется).
    pub transform: vk::SurfaceTransformFlagsKHR,
    /// Режим альфы композитора.
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
    /// `TRANSFER_DST` (+ `COLOR_ATTACHMENT`, если surface его объявила).
    pub usage: vk::ImageUsageFlags,
}

/// Режимы альфы по предпочтению: кодеку альфа не нужна, поэтому OPAQUE первым.
const ALPHA_PREFERENCE: [vk::CompositeAlphaFlagsKHR; 4] = [
    vk::CompositeAlphaFlagsKHR::OPAQUE,
    vk::CompositeAlphaFlagsKHR::INHERIT,
    vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
    vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
];

/// Собирает план swapchain цели кодера. Каждый отказ — своя ошибка, ничего не подставляется молча.
pub fn plan_encoder_swapchain(
    support: &SurfaceSupport,
    formats: &[vk::SurfaceFormatKHR],
    extent: vk::Extent2D,
) -> Result<EncoderSwapchainPlan, EncoderTargetError> {
    if extent.width == 0 || extent.height == 0 {
        return Err(EncoderTargetError::ZeroExtent);
    }
    if !support.supported_usage.contains(vk::ImageUsageFlags::TRANSFER_DST) {
        return Err(EncoderTargetError::TransferDstUnsupported {
            supported: support.supported_usage,
        });
    }
    let mut usage = vk::ImageUsageFlags::TRANSFER_DST;
    if support.supported_usage.contains(vk::ImageUsageFlags::COLOR_ATTACHMENT) {
        usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    check_extent(support, extent)?;
    let surface_format = ENCODER_FORMATS
        .iter()
        .find_map(|wanted| {
            formats
                .iter()
                .copied()
                .find(|entry| entry.format == *wanted && entry.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR)
        })
        .ok_or_else(|| EncoderTargetError::NoSuitableFormat {
            available: formats.to_vec(),
        })?;
    let composite_alpha = ALPHA_PREFERENCE
        .into_iter()
        .find(|flag| support.supported_composite_alpha.contains(*flag))
        .ok_or(EncoderTargetError::NoCompositeAlpha {
            supported: support.supported_composite_alpha,
        })?;
    // Кадр один: достаточно минимума. `max(1)` — на случай драйвера с `minImageCount = 0`.
    let image_count = support.min_image_count.max(1);
    let image_count = if support.max_image_count == 0 {
        image_count
    } else {
        image_count.min(support.max_image_count)
    };
    Ok(EncoderSwapchainPlan {
        surface_format,
        image_count,
        extent,
        transform: support.current_transform,
        composite_alpha,
        usage,
    })
}

/// Размер снимка против требований surface: определённый `currentExtent` обязан совпасть (VUID
/// `imageExtent-01274`), иначе размер должен лежать в `[min, max]`. Зажатия нет: снимок нельзя «чуть изменить».
fn check_extent(support: &SurfaceSupport, extent: vk::Extent2D) -> Result<(), EncoderTargetError> {
    if let Some(current) = support.current_extent {
        if current.width != extent.width || current.height != extent.height {
            return Err(EncoderTargetError::ExtentRejectedBySurface {
                requested: extent,
                required: current,
            });
        }
        return Ok(());
    }
    let (min, max) = (support.min_image_extent, support.max_image_extent);
    let inside = extent.width >= min.width
        && extent.height >= min.height
        && extent.width <= max.width
        && extent.height <= max.height;
    if inside {
        Ok(())
    } else {
        Err(EncoderTargetError::ExtentOutOfRange {
            requested: extent,
            min,
            max,
        })
    }
}
