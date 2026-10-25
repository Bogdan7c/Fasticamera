//! Импорт `AHardwareBuffer` в Vulkan тремя способами и запрос свойств буфера.
//!
//! Все функции возвращают ошибку текстом (`"<этап>: <VkResult>"`): отказ драйвера —
//! это *данные* отчёта, а не сбой пробы.

use ash::vk;

use super::context::VulkanContext;
use super::vk_step;

/// Ссылка на живой `AHardwareBuffer`. Создание — единственная точка, где вызывающий
/// даёт гарантию жизни буфера; дальше функции модуля остаются безопасными.
#[derive(Clone, Copy)]
pub struct AhbRef(*mut vk::AHardwareBuffer);

impl AhbRef {
    /// # Safety
    /// `ptr` — валидный `AHardwareBuffer*`, переживающий все использования этого `AhbRef`
    /// (в пробе его держит `AImage`, освобождаемый только после обработки кадра).
    pub unsafe fn from_raw(ptr: *mut vk::AHardwareBuffer) -> Self {
        Self(ptr)
    }

    fn as_ptr(self) -> *mut vk::AHardwareBuffer {
        self.0
    }
}

/// Свойства AHB с точки зрения Vulkan (`vkGetAndroidHardwareBufferPropertiesANDROID`).
pub struct AhbProperties {
    pub allocation_size: u64,
    pub memory_type_bits: u32,
    /// `UNDEFINED`, если формат у Vulkan только внешний (`external_format`).
    pub format: vk::Format,
    pub external_format: u64,
    pub format_features: vk::FormatFeatureFlags,
}

/// Запрашивает свойства буфера и format properties одним вызовом (через pNext).
pub fn query_properties(ctx: &VulkanContext, ahb: AhbRef) -> Result<AhbProperties, String> {
    let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
    let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default().push_next(&mut format_properties);
    // SAFETY: ahb — живой AHardwareBuffer (его держит AImage); структуры локальные.
    vk_step(
        unsafe {
            ctx.ahb_loader
                .get_android_hardware_buffer_properties(ahb.as_ptr(), &mut properties)
        },
        "vkGetAndroidHardwareBufferPropertiesANDROID",
    )?;
    let allocation_size = properties.allocation_size;
    let memory_type_bits = properties.memory_type_bits;
    Ok(AhbProperties {
        allocation_size,
        memory_type_bits,
        format: format_properties.format,
        external_format: format_properties.external_format,
        format_features: format_properties.format_features,
    })
}

/// VkImage + импортированная память. Уничтожается явно (`destroy`) или передаётся в корзину кадра.
pub struct ImportedImage {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
}

impl ImportedImage {
    /// Немедленное уничтожение (для проверочных импортов, которые дальше не нужны).
    pub fn destroy(self, ctx: &VulkanContext) {
        // SAFETY: объекты созданы на ctx.device, GPU их не использовал.
        unsafe {
            ctx.device.destroy_image(self.image, None);
            ctx.device.free_memory(self.memory, None);
        }
    }
}

/// Какой образ создаём.
pub enum ImageKind {
    /// Обычный формат — можно читать из compute-шейдера как storage image.
    Storage(vk::Format),
    /// Внешний формат (`VkExternalFormatANDROID`) — только sampled через YCbCr; проверяем создание/биндинг.
    ExternalFormat,
}

/// Геометрия буфера для создания образа.
pub struct ImageGeometry {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
}

/// Импорт AHB как `VkImage` + `vkAllocateMemory` с `ImportAndroidHardwareBufferInfoANDROID` + bind.
pub fn import_as_image(
    ctx: &VulkanContext,
    ahb: AhbRef,
    properties: &AhbProperties,
    geometry: &ImageGeometry,
    kind: &ImageKind,
) -> Result<ImportedImage, String> {
    let (format, usage) = match kind {
        ImageKind::Storage(format) => (*format, vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED),
        ImageKind::ExternalFormat => (vk::Format::UNDEFINED, vk::ImageUsageFlags::SAMPLED),
    };
    let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    let mut external_format = vk::ExternalFormatANDROID::default().external_format(properties.external_format);
    let mut image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: geometry.width,
            height: geometry.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(geometry.layers.max(1))
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_memory);
    if matches!(kind, ImageKind::ExternalFormat) {
        image_info = image_info.push_next(&mut external_format);
    }

    // SAFETY: image_info и вложенные структуры живут до конца вызова.
    let image = vk_step(unsafe { ctx.device.create_image(&image_info, None) }, "vkCreateImage")?;
    // SAFETY: image только что создан на этом device.
    let requirements = unsafe { ctx.device.get_image_memory_requirements(image) };
    let allocation = allocate_imported_memory(
        ctx,
        ahb,
        properties,
        requirements.memory_type_bits,
        DedicatedTarget::Image(image),
    );
    let memory = match allocation {
        Ok(memory) => memory,
        Err(error) => {
            // SAFETY: image создан выше и нигде не использован.
            unsafe { ctx.device.destroy_image(image, None) };
            return Err(error);
        }
    };
    // SAFETY: image и memory принадлежат одному device; память импортирована под этот образ.
    if let Err(error) = vk_step(
        unsafe { ctx.device.bind_image_memory(image, memory, 0) },
        "vkBindImageMemory",
    ) {
        ImportedImage { image, memory }.destroy(ctx);
        return Err(error);
    }
    Ok(ImportedImage { image, memory })
}

/// VkBuffer + импортированная память.
pub struct ImportedBuffer {
    pub buffer: vk::Buffer,
    pub memory: vk::DeviceMemory,
    pub size: u64,
}

impl ImportedBuffer {
    pub fn destroy(self, ctx: &VulkanContext) {
        // SAFETY: объекты созданы на ctx.device, GPU их не использовал.
        unsafe {
            ctx.device.destroy_buffer(self.buffer, None);
            ctx.device.free_memory(self.memory, None);
        }
    }
}

/// Импорт AHB как `VkBuffer` (размер = `allocationSize`). Спецификация разрешает это
/// не для всех форматов AHB (классически — для BLOB); отказ драйвера попадает в отчёт.
pub fn import_as_buffer(
    ctx: &VulkanContext,
    ahb: AhbRef,
    properties: &AhbProperties,
) -> Result<ImportedBuffer, String> {
    let mut external_memory = vk::ExternalMemoryBufferCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    let buffer_info = vk::BufferCreateInfo::default()
        .size(properties.allocation_size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut external_memory);
    // SAFETY: buffer_info и external_memory живут до конца вызова.
    let buffer = vk_step(
        unsafe { ctx.device.create_buffer(&buffer_info, None) },
        "vkCreateBuffer",
    )?;
    // SAFETY: buffer только что создан.
    let requirements = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
    let memory = match allocate_imported_memory(
        ctx,
        ahb,
        properties,
        requirements.memory_type_bits,
        DedicatedTarget::Buffer(buffer),
    ) {
        Ok(memory) => memory,
        Err(error) => {
            // SAFETY: buffer создан выше и не использован.
            unsafe { ctx.device.destroy_buffer(buffer, None) };
            return Err(error);
        }
    };
    // SAFETY: buffer и memory на одном device.
    if let Err(error) = vk_step(
        unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0) },
        "vkBindBufferMemory",
    ) {
        ImportedBuffer {
            buffer,
            memory,
            size: properties.allocation_size,
        }
        .destroy(ctx);
        return Err(error);
    }
    Ok(ImportedBuffer {
        buffer,
        memory,
        size: properties.allocation_size,
    })
}

/// Для кого выделяется dedicated-память.
enum DedicatedTarget {
    Image(vk::Image),
    Buffer(vk::Buffer),
}

/// `vkAllocateMemory` с импортом AHB. Тип памяти — пересечение масок AHB и ресурса.
fn allocate_imported_memory(
    ctx: &VulkanContext,
    ahb: AhbRef,
    properties: &AhbProperties,
    resource_memory_bits: u32,
    target: DedicatedTarget,
) -> Result<vk::DeviceMemory, String> {
    let allowed_bits = properties.memory_type_bits & resource_memory_bits;
    let memory_type_index = ctx
        .find_memory_type(allowed_bits, vk::MemoryPropertyFlags::empty())
        .ok_or_else(|| {
            format!(
                "нет общего типа памяти: ahb_bits=0x{:X} resource_bits=0x{resource_memory_bits:X}",
                properties.memory_type_bits
            )
        })?;
    let mut import_info = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(ahb.as_ptr());
    let mut dedicated = match target {
        DedicatedTarget::Image(image) => vk::MemoryDedicatedAllocateInfo::default().image(image),
        DedicatedTarget::Buffer(buffer) => vk::MemoryDedicatedAllocateInfo::default().buffer(buffer),
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(properties.allocation_size)
        .memory_type_index(memory_type_index)
        .push_next(&mut import_info)
        .push_next(&mut dedicated);
    // SAFETY: allocate_info и цепочка pNext живут до конца вызова; ahb жив (держит AImage).
    vk_step(
        unsafe { ctx.device.allocate_memory(&allocate_info, None) },
        "vkAllocateMemory(import AHB)",
    )
}

/// Создаёт `VkImageView` формата R16_UINT над импортированным образом.
pub fn create_r16_uint_view(ctx: &VulkanContext, image: vk::Image) -> Result<vk::ImageView, String> {
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R16_UINT)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    // SAFETY: image принадлежит этому device и уже привязан к памяти.
    vk_step(
        unsafe { ctx.device.create_image_view(&view_info, None) },
        "vkCreateImageView",
    )
}
