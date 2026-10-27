//! Выбор формата storage-картинки по возможностям устройства — чистая логика.

use ash::vk;

use crate::error::GpuError;

/// Формат storage-картинки. От него зависит квалификатор формата в шейдере, поэтому вызывающий
/// выбирает вариант SPIR-V по [`StorageImageFormat::glsl_format_qualifier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageImageFormat {
    /// `A2B10G10R10_UNORM_PACK32`: 10 бит на канал, 4 байта на пиксель — вдвое меньше трафика, чем fp16.
    /// Требует `shaderStorageImageExtendedFormats` (квалификатор `rgb10_a2`).
    A2B10G10R10Unorm,
    /// `R16G16B16A16_SFLOAT`: базовый storage-формат (есть везде), 8 байт на пиксель.
    Rgba16Float,
}

impl StorageImageFormat {
    /// Формат Vulkan.
    pub fn vk_format(self) -> vk::Format {
        match self {
            Self::A2B10G10R10Unorm => vk::Format::A2B10G10R10_UNORM_PACK32,
            Self::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        }
    }

    /// Квалификатор формата GLSL для `layout(..., <квалификатор>) uniform writeonly image2D`.
    pub fn glsl_format_qualifier(self) -> &'static str {
        match self {
            Self::A2B10G10R10Unorm => "rgb10_a2",
            Self::Rgba16Float => "rgba16f",
        }
    }

    /// Байт на пиксель.
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            Self::A2B10G10R10Unorm => 4,
            Self::Rgba16Float => 8,
        }
    }
}

/// Какой формат предпочесть.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageFormatPolicy {
    /// 10-бит UNORM, если устройство умеет писать его как storage image; иначе fp16.
    #[default]
    PreferTenBitUnorm,
    /// Только fp16 (например, для паритетных тестов с большей точностью или при отладке).
    HalfFloatOnly,
}

/// Возможности формата, без которых storage-картинка видоискателя бесполезна: запись в compute,
/// чтение с линейной фильтрацией (компоновщик, стекло) и копирование (readback, диагностика).
pub(crate) const REQUIRED_FEATURES: vk::FormatFeatureFlags = vk::FormatFeatureFlags::from_raw(
    vk::FormatFeatureFlags::STORAGE_IMAGE.as_raw()
        | vk::FormatFeatureFlags::SAMPLED_IMAGE.as_raw()
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR.as_raw()
        | vk::FormatFeatureFlags::TRANSFER_SRC.as_raw(),
);

/// Что устройство умеет с кандидатами (снимок `vkGetPhysicalDeviceFormatProperties`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StorageFormatSupport {
    /// Включена ли `shaderStorageImageExtendedFormats` (без неё `rgb10_a2` в шейдере запрещён).
    pub extended_formats_enabled: bool,
    /// `optimalTilingFeatures` формата `A2B10G10R10_UNORM_PACK32`.
    pub ten_bit_features: vk::FormatFeatureFlags,
    /// `optimalTilingFeatures` формата `R16G16B16A16_SFLOAT`.
    pub half_float_features: vk::FormatFeatureFlags,
}

/// Выбирает формат. Нет ни одного подходящего — `Unsupported` с перечнем того, чего не хватает.
pub(crate) fn choose_storage_format(
    policy: StorageFormatPolicy,
    support: StorageFormatSupport,
) -> Result<StorageImageFormat, GpuError> {
    let ten_bit_usable = support.extended_formats_enabled && support.ten_bit_features.contains(REQUIRED_FEATURES);
    if policy == StorageFormatPolicy::PreferTenBitUnorm && ten_bit_usable {
        return Ok(StorageImageFormat::A2B10G10R10Unorm);
    }
    if support.half_float_features.contains(REQUIRED_FEATURES) {
        return Ok(StorageImageFormat::Rgba16Float);
    }
    Err(GpuError::Unsupported {
        what: format!(
            "нет формата storage-картинки с {REQUIRED_FEATURES:?}: 10 бит {:?} (extended formats {}), fp16 {:?}",
            support.ten_bit_features, support.extended_formats_enabled, support.half_float_features
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn support(extended: bool, ten_bit: vk::FormatFeatureFlags, half: vk::FormatFeatureFlags) -> StorageFormatSupport {
        StorageFormatSupport {
            extended_formats_enabled: extended,
            ten_bit_features: ten_bit,
            half_float_features: half,
        }
    }

    #[test]
    fn ten_bit_is_preferred_when_storage_capable_and_extended_formats_enabled() {
        let chosen = choose_storage_format(
            StorageFormatPolicy::PreferTenBitUnorm,
            support(true, REQUIRED_FEATURES, REQUIRED_FEATURES),
        );
        assert_eq!(chosen, Ok(StorageImageFormat::A2B10G10R10Unorm));
    }

    #[test]
    fn falls_back_to_half_float_without_extended_formats_or_storage_bit() {
        let without_extended = choose_storage_format(
            StorageFormatPolicy::PreferTenBitUnorm,
            support(false, REQUIRED_FEATURES, REQUIRED_FEATURES),
        );
        assert_eq!(without_extended, Ok(StorageImageFormat::Rgba16Float));
        let no_storage_bit = REQUIRED_FEATURES & !vk::FormatFeatureFlags::STORAGE_IMAGE;
        let without_storage = choose_storage_format(
            StorageFormatPolicy::PreferTenBitUnorm,
            support(true, no_storage_bit, REQUIRED_FEATURES),
        );
        assert_eq!(without_storage, Ok(StorageImageFormat::Rgba16Float));
    }

    #[test]
    fn half_float_only_policy_ignores_ten_bit() {
        let chosen = choose_storage_format(
            StorageFormatPolicy::HalfFloatOnly,
            support(true, REQUIRED_FEATURES, REQUIRED_FEATURES),
        );
        assert_eq!(chosen, Ok(StorageImageFormat::Rgba16Float));
    }

    #[test]
    fn nothing_usable_is_an_explicit_unsupported_error() {
        let no_linear = REQUIRED_FEATURES & !vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR;
        let error = choose_storage_format(
            StorageFormatPolicy::PreferTenBitUnorm,
            support(true, no_linear, no_linear),
        )
        .unwrap_err();
        assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
    }

    #[test]
    fn qualifiers_and_sizes_match_formats() {
        assert_eq!(StorageImageFormat::A2B10G10R10Unorm.glsl_format_qualifier(), "rgb10_a2");
        assert_eq!(StorageImageFormat::Rgba16Float.glsl_format_qualifier(), "rgba16f");
        assert_eq!(StorageImageFormat::A2B10G10R10Unorm.bytes_per_texel(), 4);
        assert_eq!(StorageImageFormat::Rgba16Float.bytes_per_texel(), 8);
    }
}
