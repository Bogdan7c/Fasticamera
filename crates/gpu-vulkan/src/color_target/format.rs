//! Форматы промежуточных цветовых целей: что поддерживаем и как читаем обратно.

use ash::vk;

/// Формат [`super::ColorTarget`]. Только UNORM/fp16 без sRGB-кодирования: значения шейдера пишутся
/// как есть (то же правило, что у цели показа и компоновщика).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTargetFormat {
    /// 8 бит на канал (`R8G8B8A8_UNORM`).
    Rgba8Unorm,
    /// 10 бит на цветовой канал, 2 бита альфы (`A2B10G10R10_UNORM_PACK32`): как цель показа HDR10-класса.
    A2B10G10R10Unorm,
    /// Половинная точность (`R16G16B16A16_SFLOAT`): ступени пирамиды размытия без квантования.
    Rgba16Float,
}

impl ColorTargetFormat {
    /// Формат Vulkan.
    pub fn vk_format(self) -> vk::Format {
        match self {
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
            Self::A2B10G10R10Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
            Self::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        }
    }

    /// Байт на тексель (плотная упаковка при readback).
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            Self::Rgba8Unorm | Self::A2B10G10R10Unorm => 4,
            Self::Rgba16Float => 8,
        }
    }

    /// Возможности формата, без которых цель не годится: рисование со смешиванием, чтение сэмплером
    /// с линейной фильтрацией (пирамида размытия) и копирование (readback).
    pub(crate) fn required_features() -> vk::FormatFeatureFlags {
        vk::FormatFeatureFlags::COLOR_ATTACHMENT
            | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
            | vk::FormatFeatureFlags::SAMPLED_IMAGE
            | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
            | vk::FormatFeatureFlags::TRANSFER_SRC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_map_to_expected_vulkan_formats_and_sizes() {
        assert_eq!(ColorTargetFormat::Rgba8Unorm.vk_format(), vk::Format::R8G8B8A8_UNORM);
        assert_eq!(
            ColorTargetFormat::A2B10G10R10Unorm.vk_format(),
            vk::Format::A2B10G10R10_UNORM_PACK32
        );
        assert_eq!(
            ColorTargetFormat::Rgba16Float.vk_format(),
            vk::Format::R16G16B16A16_SFLOAT
        );
        assert_eq!(ColorTargetFormat::Rgba8Unorm.bytes_per_texel(), 4);
        assert_eq!(ColorTargetFormat::A2B10G10R10Unorm.bytes_per_texel(), 4);
        assert_eq!(ColorTargetFormat::Rgba16Float.bytes_per_texel(), 8);
    }
}
