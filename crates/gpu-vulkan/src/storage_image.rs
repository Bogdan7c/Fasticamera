//! [`StorageImage`] — картинка, которую compute-шейдер пишет, а фрагментный (или другой compute)
//! читает с линейной фильтрацией. Выход ISP видоискателя (сессия 07) и источник `BackdropSource`.
//!
//! Формат выбирается по возможностям устройства ([`StorageFormatPolicy`]): 10-бит UNORM, если его
//! можно писать как storage image, иначе fp16. Выбранный формат сообщается ([`StorageImage::format`]),
//! потому что квалификатор формата в шейдере должен с ним совпадать.
//!
//! Раскладку картинка НЕ отслеживает сама: барьеры пишутся в командные буферы, которые исполнятся
//! позже, поэтому «что было до» передаёт вызывающий ([`StorageImagePreviousUse`]) — он знает порядок
//! своих проходов. Переходы — intent-методы `record_*` (см. [`transitions`]).

mod format;
pub(crate) mod readback;
mod transitions;

use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;

pub use self::format::{StorageFormatPolicy, StorageImageFormat};
use self::format::{StorageFormatSupport, choose_storage_format};
pub use self::transitions::StorageImagePreviousUse;
use self::transitions::{COMPUTE_WRITE, record_image_barrier, sampled_read};
use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;
use crate::live_objects::{OwnedDeviceMemory, OwnedImage, OwnedImageView};

/// Как картинку будут использовать. Сейчас один сценарий; enum оставляет место для других
/// (например, «только compute → compute» без линейной фильтрации) без смены сигнатуры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageImageUsage {
    /// Запись в compute, чтение как sampled image (фрагмент/compute) с линейной фильтрацией,
    /// копирование для readback. Usage: `STORAGE | SAMPLED | TRANSFER_SRC`.
    #[default]
    ComputeWriteSampledRead,
}

impl StorageImageUsage {
    fn vk_usage(self) -> vk::ImageUsageFlags {
        match self {
            Self::ComputeWriteSampledRead => {
                vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC
            }
        }
    }
}

/// Описание storage-картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageImageDesc {
    /// Размер в пикселях (оба > 0 и ≤ `maxImageDimension2D`).
    pub extent: vk::Extent2D,
    /// Предпочтение формата.
    pub format_policy: StorageFormatPolicy,
    /// Сценарий использования.
    pub usage: StorageImageUsage,
}

/// Представление для чтения (компоновщик, стекло): что и какого размера читать.
/// Раскладка при чтении — `SHADER_READ_ONLY_OPTIMAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampledImageView {
    /// Представление с usage `SAMPLED`.
    pub view: vk::ImageView,
    /// Размер картинки, пиксели.
    pub extent: vk::Extent2D,
    /// Формат картинки.
    pub format: vk::Format,
}

/// Поколение storage-картинки: уникальный номер на процесс, выдаётся при создании и не повторяется.
///
/// Нужен тем, кто держит «голые» хэндлы картинки (`BackdropSource`): по нему владелец отвечает,
/// жива ли ещё именно ЭТА картинка, — хэндл Vulkan после уничтожения может быть переиспользован
/// драйвером для новой картинки, а поколение — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageImageGeneration(pub u64);

/// Счётчик поколений на процесс (0 не выдаётся).
static NEXT_STORAGE_IMAGE_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Storage-картинка с памятью GPU. Порядок полей = порядок уничтожения: представление → картинка → память.
pub struct StorageImage {
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
    extent: vk::Extent2D,
    format: StorageImageFormat,
    generation: StorageImageGeneration,
    context: GpuContext,
}

impl std::fmt::Debug for StorageImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "StorageImage({}x{}, {:?})",
            self.extent.width, self.extent.height, self.format
        )
    }
}

impl StorageImage {
    /// Создаёт картинку. Нулевой размер и размер сверх лимита — `InvalidArgument`; нет подходящего
    /// формата — `Unsupported`. Содержимое не определено (первый переход — из
    /// [`StorageImagePreviousUse::NotUsedYet`]).
    pub fn new(context: &GpuContext, desc: &StorageImageDesc) -> Result<Self, GpuError> {
        let limit = context.max_image_dimension_2d();
        let vk::Extent2D { width, height } = desc.extent;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(GpuError::InvalidArgument {
                what: format!("размер storage-картинки {width}x{height} вне 1..={limit}"),
            });
        }
        let support = StorageFormatSupport {
            extended_formats_enabled: context.capabilities().storage_image_extended_formats,
            ten_bit_features: context.optimal_tiling_features(StorageImageFormat::A2B10G10R10Unorm.vk_format()),
            half_float_features: context.optimal_tiling_features(StorageImageFormat::Rgba16Float.vk_format()),
        };
        let format = choose_storage_format(desc.format_policy, support)?;
        let device = context.device();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format.vk_format())
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(desc.usage.vk_usage())
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — живая локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(
                unsafe { device.create_image(&image_info, None) },
                "vkCreateImage(storage)",
            )?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "storage-картинка")?;
        // SAFETY: память подобрана по требованиям картинки и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory(storage)",
        )?;
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image.handle())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format.vk_format())
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
                "vkCreateImageView(storage)",
            )?,
        );
        Ok(Self {
            view,
            image,
            _memory: memory,
            extent: desc.extent,
            format,
            // Relaxed достаточно: нужна только уникальность номера, упорядочивать с ним нечего.
            generation: StorageImageGeneration(NEXT_STORAGE_IMAGE_GENERATION.fetch_add(1, Ordering::Relaxed)),
            context: context.clone(),
        })
    }

    /// Поколение картинки (уникально на процесс; см. [`StorageImageGeneration`]).
    pub fn generation(&self) -> StorageImageGeneration {
        self.generation
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Выбранный формат (по нему выбирается вариант шейдера с нужным квалификатором).
    pub fn format(&self) -> StorageImageFormat {
        self.format
    }

    /// Ресурс для привязки [`crate::compute::BindingKind::StorageImageWrite`] (раскладка `GENERAL`).
    pub fn storage_binding(&self) -> DescriptorResource {
        DescriptorResource::StorageImage {
            view: self.view.handle(),
        }
    }

    /// Ресурс для чтения в compute (`SampledImage`/`CombinedImageSampler`, раскладка `SHADER_READ_ONLY_OPTIMAL`).
    pub fn read_only_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadOnlyImage {
            view: self.view.handle(),
        }
    }

    /// Представление для чтения компоновщиком/стеклом.
    pub fn sampled_view(&self) -> SampledImageView {
        SampledImageView {
            view: self.view.handle(),
            extent: self.extent,
            format: self.format.vk_format(),
        }
    }

    /// Готовит картинку к записи в compute после `previous` (см. WAR в [`transitions`]).
    pub fn record_prepare_for_compute_write(
        &self,
        command_buffer: vk::CommandBuffer,
        previous: StorageImagePreviousUse,
    ) {
        record_image_barrier(
            self.context.device(),
            command_buffer,
            self.image.handle(),
            previous.source_scope(),
            COMPUTE_WRITE,
        );
    }

    /// Готовит картинку к записи в compute, дожидаясь чтений на ВСЕХ шейдерных стадиях (фрагмент и
    /// compute), если `previous` — чтение. Для картинки с несколькими читателями, которых владелец не
    /// отслеживает поимённо (выход ISP: компоновщик + подложка стекла), см. [`transitions`].
    pub fn record_prepare_for_compute_write_after_any_reader(
        &self,
        command_buffer: vk::CommandBuffer,
        previous: StorageImagePreviousUse,
    ) {
        record_image_barrier(
            self.context.device(),
            command_buffer,
            self.image.handle(),
            previous.source_scope_after_any_shader_reader(),
            COMPUTE_WRITE,
        );
    }

    /// Записанное compute-шейдером отдаёт на чтение стадии `reader` (раскладка `SHADER_READ_ONLY_OPTIMAL`).
    pub fn record_compute_write_to_sampled_read(&self, command_buffer: vk::CommandBuffer, reader: ConsumerStage) {
        record_image_barrier(
            self.context.device(),
            command_buffer,
            self.image.handle(),
            StorageImagePreviousUse::ComputeWrite.source_scope(),
            sampled_read(reader),
        );
    }

    /// То же для фрагментного шейдера (компоновщик видоискателя на экран).
    pub fn record_compute_write_to_fragment_read(&self, command_buffer: vk::CommandBuffer) {
        self.record_compute_write_to_sampled_read(command_buffer, ConsumerStage::Fragment);
    }

    /// Сам `VkImage` (для барьеров внутри crate: readback, backdrop).
    pub(crate) fn image_handle(&self) -> vk::Image {
        self.image.handle()
    }

    /// Контекст-владелец (для барьеров `BackdropSource`).
    pub(crate) fn context(&self) -> &GpuContext {
        &self.context
    }
}
