//! [`SampledTexture`] — неизменяемая текстура, которую шейдеры только читают сэмплером (атлас SDF-глифов
//! UI). Данные загружаются один раз при создании; дальше текстура в `SHADER_READ_ONLY_OPTIMAL` навсегда.
//!
//! Загрузка — блокирующая разовая отправка через staging-буфер: это путь ИНИЦИАЛИЗАЦИИ, а не кадра
//! (на горячем пути ожиданий GPU быть не должно). Все проверки (размер, длина данных, возможности
//! формата) идут до создания объектов Vulkan.
//!
//! Раскладка готова для чтения стадиями `FRAGMENT` и `COMPUTE` (барьер завершает загрузку для обеих);
//! привязывается как [`crate::compute::BindingKind::CombinedImageSampler`] с
//! [`crate::compute::DescriptorResource::ReadOnlyImage`] (линейный сэмплер, края зажаты — сэмплер
//! принадлежит конвейеру).

use ash::vk;

use crate::color_target::{ImageScope, record_image_barrier};
use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::host_buffer::{HostBuffer, HostBufferUsage};
use crate::live_objects::{OwnedDeviceMemory, OwnedImage, OwnedImageView};
use crate::storage_image::SampledImageView;

/// Формат текстуры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// Один канал 8 бит (`R8_UNORM`): поле расстояний глифов.
    R8Unorm,
    /// Четыре канала по 8 бит (`R8G8B8A8_UNORM`).
    Rgba8Unorm,
}

impl TextureFormat {
    /// Формат Vulkan.
    pub fn vk_format(self) -> vk::Format {
        match self {
            Self::R8Unorm => vk::Format::R8_UNORM,
            Self::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        }
    }

    /// Байт на тексель во входных данных (плотные строки без выравнивания).
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            Self::R8Unorm => 1,
            Self::Rgba8Unorm => 4,
        }
    }
}

/// Описание текстуры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    /// Размер в пикселях (оба > 0 и ≤ `maxImageDimension2D`).
    pub extent: vk::Extent2D,
    /// Формат.
    pub format: TextureFormat,
    /// Короткое имя для ошибок (например, `"sdf-atlas"`).
    pub label: &'static str,
}

/// Текстура с памятью GPU. Порядок полей = порядок уничтожения: представление → картинка → память.
pub struct SampledTexture {
    view: OwnedImageView,
    _image: OwnedImage,
    _memory: OwnedDeviceMemory,
    extent: vk::Extent2D,
    format: TextureFormat,
}

impl std::fmt::Debug for SampledTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "SampledTexture({}x{}, {:?})",
            self.extent.width, self.extent.height, self.format
        )
    }
}

impl SampledTexture {
    /// Создаёт текстуру и загружает `texels` (построчно сверху вниз, плотно, `ширина·высота·байт` на
    /// тексель). Неверный размер или длина данных — `InvalidArgument`, формат без нужных возможностей —
    /// `Unsupported`; в обоих случаях ни один объект Vulkan не создаётся.
    pub fn upload(context: &GpuContext, desc: &TextureDesc, texels: &[u8]) -> Result<Self, GpuError> {
        validate_upload(context, desc, texels.len())?;
        let vk::Extent2D { width, height } = desc.extent;
        let vk_format = desc.format.vk_format();
        let device = context.device();

        let mut staging = HostBuffer::new(context, texels.len() as u64, HostBufferUsage::ReadOnlyStorage)?;
        staging.write_bytes(0, texels)?;

        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — живая локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(
                unsafe { device.create_image(&image_info, None) },
                "vkCreateImage(SampledTexture)",
            )?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "SampledTexture")?;
        // SAFETY: память подобрана по требованиям картинки и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory(SampledTexture)",
        )?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image.handle())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk_format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: картинка с привязанной памятью; формат совпадает.
        let view = OwnedImageView::from_raw(
            context,
            run_step(
                unsafe { device.create_image_view(&view_info, None) },
                "vkCreateImageView(SampledTexture)",
            )?,
        );
        record_upload(context, &staging, image.handle(), desc.extent)?;
        Ok(Self {
            view,
            _image: image,
            _memory: memory,
            extent: desc.extent,
            format: desc.format,
        })
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Формат.
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// Представление для чтения (раскладка `SHADER_READ_ONLY_OPTIMAL`, всегда).
    pub fn sampled_view(&self) -> SampledImageView {
        SampledImageView {
            view: self.view.handle(),
            extent: self.extent,
            format: self.format.vk_format(),
        }
    }

    /// Ресурс для привязки `CombinedImageSampler` / `SampledImage`.
    pub fn read_only_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadOnlyImage {
            view: self.view.handle(),
        }
    }
}

/// Проверки до создания объектов: размер, длина данных, возможности формата.
fn validate_upload(context: &GpuContext, desc: &TextureDesc, texel_bytes_len: usize) -> Result<(), GpuError> {
    let limit = context.max_image_dimension_2d();
    let vk::Extent2D { width, height } = desc.extent;
    if width == 0 || height == 0 || width > limit || height > limit {
        return Err(GpuError::InvalidArgument {
            what: format!("{}: размер текстуры {width}x{height} вне 1..={limit}", desc.label),
        });
    }
    let expected = expected_texel_bytes(desc);
    if texel_bytes_len as u64 != expected {
        return Err(GpuError::InvalidArgument {
            what: format!(
                "{}: данных {texel_bytes_len} байт, для {width}x{height} {:?} нужно {expected}",
                desc.label, desc.format
            ),
        });
    }
    let required = vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
        | vk::FormatFeatureFlags::TRANSFER_DST;
    let supported = context.optimal_tiling_features(desc.format.vk_format());
    if !supported.contains(required) {
        return Err(GpuError::Unsupported {
            what: format!(
                "{}: формат {:?} не поддерживает {:?} (есть {supported:?})",
                desc.label,
                desc.format,
                required & !supported
            ),
        });
    }
    Ok(())
}

/// Сколько байт данных нужно описанию (`ширина · высота · байт на тексель`, в `u64` без переполнения).
fn expected_texel_bytes(desc: &TextureDesc) -> u64 {
    u64::from(desc.extent.width) * u64::from(desc.extent.height) * u64::from(desc.format.bytes_per_texel())
}

/// Копирует staging в картинку одной блокирующей отправкой и оставляет её в `SHADER_READ_ONLY_OPTIMAL`.
fn record_upload(
    context: &GpuContext,
    staging: &HostBuffer,
    image: vk::Image,
    extent: vk::Extent2D,
) -> Result<(), GpuError> {
    let device = context.device();
    let to_transfer_dst = ImageScope {
        layout: vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        stage: vk::PipelineStageFlags2::COPY,
        access: vk::AccessFlags2::TRANSFER_WRITE,
    };
    let uninitialized = ImageScope {
        layout: vk::ImageLayout::UNDEFINED,
        stage: vk::PipelineStageFlags2::NONE,
        access: vk::AccessFlags2::NONE,
    };
    // Читать текстуру смогут и фрагментный, и compute-шейдер (SDF-атлас, стекло): ждут оба.
    let ready_for_shaders = ImageScope {
        layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        stage: vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
        access: vk::AccessFlags2::SHADER_SAMPLED_READ,
    };
    let copy_written = ImageScope {
        access: vk::AccessFlags2::TRANSFER_WRITE,
        ..to_transfer_dst
    };
    context.run_one_shot(|command_buffer| {
        record_image_barrier(device, command_buffer, image, uninitialized, to_transfer_dst);
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width: extent.width,
                height: extent.height,
                depth: 1,
            });
        // SAFETY: командный буфер в состоянии записи; картинка в TRANSFER_DST_OPTIMAL (барьер выше);
        // staging вмещает width·height·texel байт плотными строками (проверено в `validate_upload`).
        unsafe {
            device.cmd_copy_buffer_to_image(
                command_buffer,
                staging.buffer_handle(),
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        record_image_barrier(device, command_buffer, image, copy_written, ready_for_shaders);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(width: u32, height: u32, format: TextureFormat) -> TextureDesc {
        TextureDesc {
            extent: vk::Extent2D { width, height },
            format,
            label: "unit",
        }
    }

    #[test]
    fn expected_bytes_follow_format_and_extent_without_overflow() {
        assert_eq!(expected_texel_bytes(&desc(4, 3, TextureFormat::R8Unorm)), 12);
        assert_eq!(expected_texel_bytes(&desc(4, 3, TextureFormat::Rgba8Unorm)), 48);
        // Лимит размера картинки (≤ 2^16..2^17) проверяется раньше, но и на 65535² 32 бит не хватило бы.
        assert_eq!(
            expected_texel_bytes(&desc(65_535, 65_535, TextureFormat::Rgba8Unorm)),
            65_535_u64 * 65_535 * 4
        );
    }
}
