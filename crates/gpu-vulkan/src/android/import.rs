//! Импорт `AHardwareBuffer` в Vulkan без копий (решение 32, факт 00 раздел C4).
//!
//! Два пути, оба проверены на Adreno 740 зондом:
//! * [`ImportLayout::SampledImageR16Uint`] — RAW16 как `VkImage R16_UINT` (sampled image: читается `texelFetch`);
//! * [`ImportLayout::PackedBuffer`] — упакованный RAW10 как `VkBuffer` (картинкой он не
//!   импортируется, дефект 5b); распаковку делает шейдер.
//!
//! Порядок шагов (каждый — отдельная причина отказа, а не общий `bool`):
//! 1. свойства буфера для Vulkan (`vkGetAndroidHardwareBufferPropertiesANDROID` + цепочка форматов);
//! 2. НАША проверка размера раскладки против `allocationSize` (драйвер слишком большой bind не
//!    отвергает — дефект 5): [`HardwareBufferImportError::Layout`];
//! 3. ресурс с `ExternalMemory*CreateInfo` (handle type `ANDROID_HARDWARE_BUFFER`);
//! 4. проверка `memoryRequirements.size ≤ allocationSize` и пересечение `memoryTypeBits`;
//! 5. dedicated-выделение памяти с импортом AHB и bind;
//! 6. для картинки — `VkImageView`.
//!
//! Порядок уничтожения (поля структуры объявлены в нём): представление → картинка/буфер →
//! память → ссылка на AHB. Память не освобождается раньше ресурса, а AHB не умирает, пока на него
//! ссылается память. Что GPU закончил работу с импортом — обязанность владельца (кэш откладывает
//! уничтожение до серии завершения).

use ash::vk;

use super::hardware_buffer::{HardwareBufferError, HardwareBufferRef};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::hardware_buffer_cache::layout::{LayoutError, pick_import_memory_type};
use crate::hardware_buffer_cache::queue_handoff::{
    BufferAcquire, ImageAcquire, ReleaseTarget, record_buffer_acquire, record_buffer_release, record_image_acquire,
    record_image_release,
};
use crate::hardware_buffer_cache::{HardwareBufferId, ImportLayout};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory, OwnedImage, OwnedImageView};

/// Почему импорт не удался.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum HardwareBufferImportError {
    /// Ошибка Vulkan / контекста (код и шаг внутри).
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// Раскладка не влезает в аллокацию (наша проверка).
    #[error("раскладка не подходит буферу: {0}")]
    Layout(#[from] LayoutError),
    /// Ошибка NDK при работе с буфером (id и т.п.).
    #[error(transparent)]
    Buffer(#[from] HardwareBufferError),
    /// Нет типа памяти, разрешённого и AHB, и ресурсом одновременно.
    #[error(
        "нет общего типа памяти: маска AHB {ahb_memory_type_bits:#x}, маска ресурса {resource_memory_type_bits:#x}"
    )]
    NoCommonMemoryType {
        /// `memoryTypeBits` из свойств AHB.
        ahb_memory_type_bits: u32,
        /// `memoryTypeBits` из `vkGet*MemoryRequirements` ресурса.
        resource_memory_type_bits: u32,
    },
}

/// Свойства AHB с точки зрения Vulkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareBufferVulkanProperties {
    /// `allocationSize` — размер памяти, которую надо передать при импорте.
    pub allocation_size: u64,
    /// `memoryTypeBits` — подходящие типы памяти.
    pub memory_type_bits: u32,
    /// Формат Vulkan; `UNDEFINED` — у AHB только внешний формат (RAW16/RAW10 на телефоне).
    pub format: vk::Format,
    /// Внешний формат (`externalFormat`), 0 — нет.
    pub external_format: u64,
    /// Возможности формата.
    pub format_features: vk::FormatFeatureFlags,
}

/// Сырые дескрипторы импортированного ресурса (владение остаётся у [`ImportedHardwareBuffer`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedHandles {
    /// Картинка `R16_UINT` и её представление.
    SampledImage {
        /// Представление.
        view: vk::ImageView,
        /// Картинка.
        image: vk::Image,
    },
    /// Буфер на всю аллокацию.
    PackedBuffer {
        /// Буфер.
        buffer: vk::Buffer,
        /// Размер буфера, байт (= `allocationSize`).
        size: u64,
    },
}

/// Ресурс импорта. Порядок полей в варианте = порядок уничтожения.
enum ImportedResource {
    SampledImage { view: OwnedImageView, image: OwnedImage },
    PackedBuffer { buffer: OwnedBuffer, size: u64 },
}

/// Импортированный `AHardwareBuffer`: ресурс Vulkan поверх той же памяти, без копий.
pub struct ImportedHardwareBuffer {
    // ПОРЯДОК ПОЛЕЙ = ПОРЯДОК УНИЧТОЖЕНИЯ: ресурс → память → ссылка на AHB.
    resource: ImportedResource,
    _memory: OwnedDeviceMemory,
    _hardware_buffer: HardwareBufferRef,
    id: HardwareBufferId,
    layout: ImportLayout,
    properties: HardwareBufferVulkanProperties,
    context: GpuContext,
}

impl ImportedHardwareBuffer {
    /// Импортирует `hardware_buffer` с раскладкой `layout`. Берёт собственную ссылку на буфер
    /// (`Clone`), вызывающий свою сохраняет.
    pub fn import(
        context: &GpuContext,
        hardware_buffer: &HardwareBufferRef,
        layout: ImportLayout,
    ) -> Result<Self, HardwareBufferImportError> {
        let id = hardware_buffer.id()?;
        let properties = query_hardware_buffer_properties(context, hardware_buffer)?;
        // Дефект 5: драйвер не отвергает bind больше аллокации — проверяем сами, ДО создания ресурса.
        layout.check_fits(properties.allocation_size)?;

        let (resource, memory) = match layout {
            ImportLayout::SampledImageR16Uint { width, height } => {
                import_storage_image(context, hardware_buffer, &properties, width, height)?
            }
            ImportLayout::PackedBuffer { .. } => import_packed_buffer(context, hardware_buffer, &properties)?,
        };
        tracing::debug!(
            target: "fasticamera::gpu",
            ?id, ?layout, allocation_size = properties.allocation_size,
            "AHardwareBuffer импортирован"
        );
        Ok(Self {
            resource,
            _memory: memory,
            _hardware_buffer: hardware_buffer.clone(),
            id,
            layout,
            properties,
            context: context.clone(),
        })
    }

    /// Идентификатор буфера (ключ кэша).
    pub fn id(&self) -> HardwareBufferId {
        self.id
    }

    /// Раскладка, с которой сделан импорт.
    pub fn layout(&self) -> ImportLayout {
        self.layout
    }

    /// Свойства буфера для Vulkan (для отчётов).
    pub fn properties(&self) -> HardwareBufferVulkanProperties {
        self.properties
    }

    /// Сырые дескрипторы ресурса.
    pub fn handles(&self) -> ImportedHandles {
        match &self.resource {
            ImportedResource::SampledImage { view, image } => ImportedHandles::SampledImage {
                view: view.handle(),
                image: image.handle(),
            },
            ImportedResource::PackedBuffer { buffer, size } => ImportedHandles::PackedBuffer {
                buffer: buffer.handle(),
                size: *size,
            },
        }
    }

    /// Записывает acquire ресурса у внешнего владельца (камера/gralloc) в командный буфер.
    /// Для картинки итоговая раскладка — `SHADER_READ_ONLY_OPTIMAL`.
    pub fn record_acquire_from_foreign(&self, command_buffer: vk::CommandBuffer) {
        let device = self.context.device();
        let family = self.context.queue_family_index();
        match self.handles() {
            ImportedHandles::SampledImage { image, .. } => {
                record_image_acquire(device, command_buffer, image, ImageAcquire::FromForeignQueue, family);
            }
            ImportedHandles::PackedBuffer { buffer, .. } => {
                record_buffer_acquire(device, command_buffer, buffer, BufferAcquire::FromForeignQueue, family);
            }
        }
    }

    /// Записывает release ресурса внешнему владельцу (после чтения GPU).
    pub fn record_release_to_foreign(&self, command_buffer: vk::CommandBuffer) {
        let device = self.context.device();
        let family = self.context.queue_family_index();
        match self.handles() {
            ImportedHandles::SampledImage { image, .. } => {
                record_image_release(device, command_buffer, image, ReleaseTarget::ForeignQueue, family);
            }
            ImportedHandles::PackedBuffer { buffer, .. } => {
                record_buffer_release(device, command_buffer, buffer, ReleaseTarget::ForeignQueue, family);
            }
        }
    }
}

impl std::fmt::Debug for ImportedHardwareBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ImportedHardwareBuffer({:?}, {:?})", self.id, self.layout)
    }
}

/// `vkGetAndroidHardwareBufferPropertiesANDROID` вместе с цепочкой format properties.
///
/// Публична, потому что раскладку RAW10 нельзя собрать без `allocationSize` (шаг строки выводится
/// по согласованности с ним), а значит свойства нужны ДО импорта.
pub fn query_hardware_buffer_properties(
    context: &GpuContext,
    hardware_buffer: &HardwareBufferRef,
) -> Result<HardwareBufferVulkanProperties, HardwareBufferImportError> {
    let loader = context.android_hardware_buffer_loader()?;
    let mut format_properties = vk::AndroidHardwareBufferFormatPropertiesANDROID::default();
    let mut properties = vk::AndroidHardwareBufferPropertiesANDROID::default().push_next(&mut format_properties);
    // SAFETY: указатель буфера валиден (держит HardwareBufferRef); структуры локальные.
    run_step(
        unsafe { loader.get_android_hardware_buffer_properties(hardware_buffer.as_ptr(), &mut properties) },
        "vkGetAndroidHardwareBufferPropertiesANDROID",
    )?;
    let allocation_size = properties.allocation_size;
    let memory_type_bits = properties.memory_type_bits;
    Ok(HardwareBufferVulkanProperties {
        allocation_size,
        memory_type_bits,
        format: format_properties.format,
        external_format: format_properties.external_format,
        format_features: format_properties.format_features,
    })
}

/// Для кого выделяется dedicated-память.
#[derive(Clone, Copy)]
enum DedicatedTarget {
    Image(vk::Image),
    Buffer(vk::Buffer),
}

/// `vkAllocateMemory` с импортом AHB (dedicated). Размер — `allocationSize` буфера; тип памяти —
/// из пересечения масок AHB и ресурса.
fn allocate_imported_memory(
    context: &GpuContext,
    hardware_buffer: &HardwareBufferRef,
    properties: &HardwareBufferVulkanProperties,
    resource_memory_type_bits: u32,
    target: DedicatedTarget,
) -> Result<OwnedDeviceMemory, HardwareBufferImportError> {
    let memory_type_index = pick_import_memory_type(properties.memory_type_bits, resource_memory_type_bits).ok_or(
        HardwareBufferImportError::NoCommonMemoryType {
            ahb_memory_type_bits: properties.memory_type_bits,
            resource_memory_type_bits,
        },
    )?;
    let mut import_info = vk::ImportAndroidHardwareBufferInfoANDROID::default().buffer(hardware_buffer.as_ptr());
    let mut dedicated = match target {
        DedicatedTarget::Image(image) => vk::MemoryDedicatedAllocateInfo::default().image(image),
        DedicatedTarget::Buffer(buffer) => vk::MemoryDedicatedAllocateInfo::default().buffer(buffer),
    };
    let allocate_info = vk::MemoryAllocateInfo::default()
        .allocation_size(properties.allocation_size)
        .memory_type_index(memory_type_index)
        .push_next(&mut import_info)
        .push_next(&mut dedicated);
    // SAFETY: allocate_info и цепочка pNext живут до конца вызова; буфер жив (держит HardwareBufferRef);
    // ресурс из `target` создан этим же устройством.
    let memory = run_step(
        unsafe { context.device().allocate_memory(&allocate_info, None) },
        "vkAllocateMemory(import AHB)",
    )?;
    Ok(OwnedDeviceMemory::from_raw(context, memory))
}

fn import_storage_image(
    context: &GpuContext,
    hardware_buffer: &HardwareBufferRef,
    properties: &HardwareBufferVulkanProperties,
    width: u32,
    height: u32,
) -> Result<(ImportedResource, OwnedDeviceMemory), HardwareBufferImportError> {
    let device = context.device();
    let mut external_memory = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::R16_UINT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut external_memory);
    // SAFETY: image_info и цепочка живут до конца вызова.
    let image = OwnedImage::from_raw(
        context,
        run_step(
            unsafe { device.create_image(&image_info, None) },
            "vkCreateImage(import AHB)",
        )?,
    );
    // Для картинки с внешней памятью AHB `vkGetImageMemoryRequirements` ДО привязки вызывать
    // нельзя (VUID-vkGetImageMemoryRequirements-image-04004: у такой картинки память ещё не
    // определена). Размер и подходящие типы памяти берём из свойств самого AHB
    // (`allocationSize`, `memoryTypeBits`); что раскладка влезает в аллокацию, уже проверено
    // `ImportLayout::check_fits` до создания картинки.
    let memory = allocate_imported_memory(
        context,
        hardware_buffer,
        properties,
        properties.memory_type_bits,
        DedicatedTarget::Image(image.handle()),
    )?;
    // SAFETY: картинка и память одного устройства; память импортирована под эту картинку (dedicated).
    run_step(
        unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
        "vkBindImageMemory(import AHB)",
    )?;
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image.handle())
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(vk::Format::R16_UINT)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        });
    // SAFETY: картинка привязана к памяти.
    let view = OwnedImageView::from_raw(
        context,
        run_step(
            unsafe { device.create_image_view(&view_info, None) },
            "vkCreateImageView(import AHB)",
        )?,
    );
    Ok((ImportedResource::SampledImage { view, image }, memory))
}

fn import_packed_buffer(
    context: &GpuContext,
    hardware_buffer: &HardwareBufferRef,
    properties: &HardwareBufferVulkanProperties,
) -> Result<(ImportedResource, OwnedDeviceMemory), HardwareBufferImportError> {
    let device = context.device();
    let mut external_memory = vk::ExternalMemoryBufferCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    // Буфер на всю аллокацию (как в зонде): раскладка уже проверена на «влезает».
    let buffer_info = vk::BufferCreateInfo::default()
        .size(properties.allocation_size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .push_next(&mut external_memory);
    // SAFETY: buffer_info и цепочка живут до конца вызова.
    let buffer = OwnedBuffer::from_raw(
        context,
        run_step(
            unsafe { device.create_buffer(&buffer_info, None) },
            "vkCreateBuffer(import AHB)",
        )?,
    );
    // SAFETY: буфер только что создан.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
    ImportLayout::check_bytes_fit(requirements.size, properties.allocation_size)?;
    let memory = allocate_imported_memory(
        context,
        hardware_buffer,
        properties,
        requirements.memory_type_bits,
        DedicatedTarget::Buffer(buffer.handle()),
    )?;
    // SAFETY: буфер и память одного устройства; память импортирована под этот буфер (dedicated).
    run_step(
        unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
        "vkBindBufferMemory(import AHB)",
    )?;
    Ok((
        ImportedResource::PackedBuffer {
            buffer,
            size: properties.allocation_size,
        },
        memory,
    ))
}
