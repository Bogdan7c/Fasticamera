//! [`SampledTexture`] — неизменяемая текстура, которую шейдеры только читают сэмплером (атлас SDF-глифов
//! UI, картинки UI). Данные загружаются один раз; дальше текстура в `SHADER_READ_ONLY_OPTIMAL` навсегда.
//!
//! Два способа загрузки:
//! * [`SampledTexture::upload`] — блокирующая разовая отправка через staging-буфер: путь ИНИЦИАЛИЗАЦИИ,
//!   а не кадра (на горячем пути ожиданий GPU быть не должно);
//! * [`SampledTexture::allocate_for_recorded_upload`] → [`PendingUploadTexture::record_upload_from`] —
//!   копия ЗАПИСЫВАЕТСЯ в командный буфер кадра, ничего не ждёт. Путь кадра (картинки UI, сессия 14g).
//!   Копии вне динамического рендеринга нельзя, поэтому вызывающий записывает загрузку до проходов.
//!
//! Все проверки (размер, длина данных, возможности формата) идут до создания объектов Vulkan и до
//! первой записанной команды.
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

/// Объекты текстуры. Порядок полей = порядок уничтожения: представление → картинка → память.
struct TextureObjects {
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
}

/// Текстура с памятью GPU.
pub struct SampledTexture {
    objects: TextureObjects,
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

/// Текстура, у которой ещё нет содержимого: память выделена, загрузка не записана. Читать её нельзя;
/// единственное, что с ней можно сделать, — записать загрузку ([`Self::record_upload_from`]) и получить
/// [`SampledTexture`]. Брошенная без записи — просто уничтожается (GPU её не видел).
pub struct PendingUploadTexture {
    objects: TextureObjects,
    desc: TextureDesc,
    context: GpuContext,
}

impl std::fmt::Debug for PendingUploadTexture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "PendingUploadTexture({}x{}, {:?})",
            self.desc.extent.width, self.desc.extent.height, self.desc.format
        )
    }
}

impl TextureDesc {
    /// Сколько байт данных (и staging-буфера) нужно текстуре: `ширина · высота · байт на тексель`.
    pub fn texel_bytes(&self) -> u64 {
        expected_texel_bytes(self)
    }
}

impl SampledTexture {
    /// Создаёт текстуру и загружает `texels` (построчно сверху вниз, плотно, `ширина·высота·байт` на
    /// тексель) БЛОКИРУЮЩЕЙ отправкой. Неверный размер или длина данных — `InvalidArgument`, формат без
    /// нужных возможностей — `Unsupported`; в обоих случаях ни один объект Vulkan не создаётся.
    pub fn upload(context: &GpuContext, desc: &TextureDesc, texels: &[u8]) -> Result<Self, GpuError> {
        validate_upload(context, desc, texels.len())?;
        let objects = create_objects(context, desc)?;
        let mut staging = HostBuffer::new(context, texels.len() as u64, HostBufferUsage::ReadOnlyStorage)?;
        staging.write_bytes(0, texels)?;
        let device = context.device();
        context.run_one_shot(|command_buffer| {
            record_upload_commands(device, command_buffer, &staging, objects.image.handle(), desc.extent);
            Ok(())
        })?;
        Ok(Self {
            objects,
            extent: desc.extent,
            format: desc.format,
        })
    }

    /// Выделяет текстуру без содержимого под загрузку, записанную в командный буфер кадра (без ожиданий).
    /// Проверки описания — до создания объектов; длину данных проверит [`PendingUploadTexture::record_upload_from`].
    pub fn allocate_for_recorded_upload(
        context: &GpuContext,
        desc: &TextureDesc,
    ) -> Result<PendingUploadTexture, GpuError> {
        validate_extent(context, desc)?;
        validate_format(context, desc)?;
        Ok(PendingUploadTexture {
            objects: create_objects(context, desc)?,
            desc: *desc,
            context: context.clone(),
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
            view: self.objects.view.handle(),
            extent: self.extent,
            format: self.format.vk_format(),
        }
    }

    /// Ресурс для привязки `CombinedImageSampler` / `SampledImage`.
    pub fn read_only_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadOnlyImage {
            view: self.objects.view.handle(),
        }
    }
}

impl PendingUploadTexture {
    /// Записывает в `command_buffer` загрузку из `staging` (плотные строки, `desc.texel_bytes()` байт с
    /// начала): барьер `UNDEFINED → TRANSFER_DST`, копия, барьер `→ SHADER_READ_ONLY_OPTIMAL` для
    /// фрагментного и compute-шейдера. Командный буфер — в состоянии записи, ВНЕ динамического рендеринга.
    ///
    /// Ничего не ждёт. Вызывающий обязан: отправить этот буфер раньше любого, который читает текстуру, и
    /// держать `staging` живым, пока fence отправки не пройден (память текстуры и staging GPU читает/пишет
    /// до этого момента). Проверки (usage и размер staging) — до первой команды: при ошибке буфер нетронут.
    pub fn record_upload_from(
        self,
        command_buffer: vk::CommandBuffer,
        staging: &HostBuffer,
    ) -> Result<SampledTexture, GpuError> {
        if staging.usage() != HostBufferUsage::ReadOnlyStorage {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "{}: staging-буфер с usage {:?} не годится источником копии (нужен ReadOnlyStorage)",
                    self.desc.label,
                    staging.usage()
                ),
            });
        }
        let expected = self.desc.texel_bytes();
        if staging.size_bytes() < expected {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "{}: в staging-буфере {} байт, для {}x{} {:?} нужно {expected}",
                    self.desc.label,
                    staging.size_bytes(),
                    self.desc.extent.width,
                    self.desc.extent.height,
                    self.desc.format
                ),
            });
        }
        record_upload_commands(
            self.context.device(),
            command_buffer,
            staging,
            self.objects.image.handle(),
            self.desc.extent,
        );
        Ok(SampledTexture {
            objects: self.objects,
            extent: self.desc.extent,
            format: self.desc.format,
        })
    }
}

/// Проверки до создания объектов: размер, длина данных, возможности формата.
fn validate_upload(context: &GpuContext, desc: &TextureDesc, texel_bytes_len: usize) -> Result<(), GpuError> {
    validate_extent(context, desc)?;
    let expected = expected_texel_bytes(desc);
    if texel_bytes_len as u64 != expected {
        let vk::Extent2D { width, height } = desc.extent;
        return Err(GpuError::InvalidArgument {
            what: format!(
                "{}: данных {texel_bytes_len} байт, для {width}x{height} {:?} нужно {expected}",
                desc.label, desc.format
            ),
        });
    }
    validate_format(context, desc)
}

/// Размер текстуры: оба измерения > 0 и в пределах устройства.
fn validate_extent(context: &GpuContext, desc: &TextureDesc) -> Result<(), GpuError> {
    let limit = context.max_image_dimension_2d();
    let vk::Extent2D { width, height } = desc.extent;
    if width == 0 || height == 0 || width > limit || height > limit {
        return Err(GpuError::InvalidArgument {
            what: format!("{}: размер текстуры {width}x{height} вне 1..={limit}", desc.label),
        });
    }
    Ok(())
}

/// Формат умеет: чтение сэмплером, линейную фильтрацию и приём копии.
fn validate_format(context: &GpuContext, desc: &TextureDesc) -> Result<(), GpuError> {
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

/// Создаёт картинку, память и представление (без содержимого; раскладка `UNDEFINED`).
fn create_objects(context: &GpuContext, desc: &TextureDesc) -> Result<TextureObjects, GpuError> {
    let vk::Extent2D { width, height } = desc.extent;
    let vk_format = desc.format.vk_format();
    let device = context.device();
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
    Ok(TextureObjects {
        view,
        image,
        _memory: memory,
    })
}

/// Сколько байт данных нужно описанию (`ширина · высота · байт на тексель`, в `u64` без переполнения).
fn expected_texel_bytes(desc: &TextureDesc) -> u64 {
    u64::from(desc.extent.width) * u64::from(desc.extent.height) * u64::from(desc.format.bytes_per_texel())
}

/// Записывает в `command_buffer` копию staging → картинка и оставляет картинку в `SHADER_READ_ONLY_OPTIMAL`.
/// Барьеры: `UNDEFINED → TRANSFER_DST`, затем `TRANSFER_DST → SHADER_READ_ONLY` (читают фрагментный и compute).
/// Вызывающий проверил: буфер в состоянии записи вне рендеринга, staging вмещает `width·height·texel` байт.
fn record_upload_commands(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    staging: &HostBuffer,
    image: vk::Image,
    extent: vk::Extent2D,
) {
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
    // staging вмещает width·height·texel байт плотными строками (проверено вызывающим).
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
