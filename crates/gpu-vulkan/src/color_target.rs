//! [`ColorTarget`] — промежуточная цветовая картинка: в неё рисует [`crate::raster::RasterPass`], потом
//! её читает сэмплер следующего прохода (ступени пирамиды размытия фона Liquid Glass) или readback.
//!
//! Раскладку цель НЕ отслеживает сама: барьеры пишутся в командные буферы, которые исполнятся позже,
//! поэтому «что было до» передаёт вызывающий ([`ColorTargetPreviousUse`]). Переходы — intent-методы
//! `record_*` (см. [`transitions`]).
//!
//! Инварианты:
//! * usage всегда `COLOR_ATTACHMENT | SAMPLED | TRANSFER_SRC`, 1 mip, 1 слой, `OPTIMAL`;
//! * формат проверяется на возможности (рисование со смешиванием, линейная выборка, копирование) до
//!   создания объектов Vulkan — нет возможности → `Unsupported`;
//! * при рисовании цель в `COLOR_ATTACHMENT_OPTIMAL`, при чтении сэмплером — в `SHADER_READ_ONLY_OPTIMAL`.

mod barriers;
mod format;
pub(crate) mod readback;
mod transitions;

use ash::vk;

pub(crate) use self::barriers::{ImageScope, record_image_barrier};
pub use self::format::ColorTargetFormat;
pub(crate) use self::transitions::transfer_read_scope;
pub use self::transitions::{ColorTargetPreviousUse, ContentOnPrepare};
pub(crate) use self::transitions::{render_scope, sampled_read_scope};
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;
use crate::live_objects::{OwnedDeviceMemory, OwnedImage, OwnedImageView};
use crate::raster::ColorAttachment;
use crate::storage_image::SampledImageView;

/// Описание цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTargetDesc {
    /// Размер в пикселях (оба > 0 и ≤ `maxImageDimension2D`).
    pub extent: vk::Extent2D,
    /// Формат.
    pub format: ColorTargetFormat,
    /// Короткое имя для ошибок (например, `"glass-blur-level-1"`).
    pub label: &'static str,
}

/// Цветовая цель с памятью GPU. Порядок полей = порядок уничтожения: представление → картинка → память.
pub struct ColorTarget {
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
    extent: vk::Extent2D,
    format: ColorTargetFormat,
    label: &'static str,
    context: GpuContext,
}

impl std::fmt::Debug for ColorTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "ColorTarget({}, {}x{}, {:?})",
            self.label, self.extent.width, self.extent.height, self.format
        )
    }
}

impl ColorTarget {
    /// Создаёт цель. Нулевой размер/сверх лимита — `InvalidArgument`; формат без нужных возможностей —
    /// `Unsupported`. Все проверки идут до создания объектов. Содержимое не определено (первый переход —
    /// из [`ColorTargetPreviousUse::NotUsedYet`]).
    pub fn new(context: &GpuContext, desc: &ColorTargetDesc) -> Result<Self, GpuError> {
        let limit = context.max_image_dimension_2d();
        let vk::Extent2D { width, height } = desc.extent;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(GpuError::InvalidArgument {
                what: format!("{}: размер цели {width}x{height} вне 1..={limit}", desc.label),
            });
        }
        let vk_format = desc.format.vk_format();
        let required = ColorTargetFormat::required_features();
        let supported = context.optimal_tiling_features(vk_format);
        if !supported.contains(required) {
            return Err(GpuError::Unsupported {
                what: format!(
                    "{}: формат {vk_format:?} не поддерживает {:?} (есть {supported:?})",
                    desc.label,
                    required & !supported
                ),
            });
        }
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
            .usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — живая локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(
                unsafe { device.create_image(&image_info, None) },
                "vkCreateImage(ColorTarget)",
            )?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "ColorTarget")?;
        // SAFETY: память подобрана по требованиям картинки и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory(ColorTarget)",
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
                "vkCreateImageView(ColorTarget)",
            )?,
        );
        Ok(Self {
            view,
            image,
            _memory: memory,
            extent: desc.extent,
            format: desc.format,
            label: desc.label,
            context: context.clone(),
        })
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Формат Vulkan (по нему создаётся `RasterPipeline` под эту цель).
    pub fn format(&self) -> vk::Format {
        self.format.vk_format()
    }

    /// Цель рисования для [`crate::raster::RasterPass::begin`]. Цель обязана быть подготовлена
    /// [`Self::record_prepare_for_render`] в том же или предыдущем командном буфере.
    pub fn attachment(&self) -> ColorAttachment {
        ColorAttachment::from_parts(self.view.handle(), self.extent, self.format.vk_format())
    }

    /// Представление для чтения сэмплером (раскладка `SHADER_READ_ONLY_OPTIMAL`, см.
    /// [`Self::record_render_to_sampled_read`]).
    pub fn sampled_view(&self) -> SampledImageView {
        SampledImageView {
            view: self.view.handle(),
            extent: self.extent,
            format: self.format.vk_format(),
        }
    }

    /// Готовит цель к рисованию после `previous`. С [`ContentOnPrepare::Preserve`] прежнее содержимое
    /// сохраняется; но «сохранить» цель, которую ещё не использовали (`NotUsedYet`), нечего —
    /// `InvalidArgument` до записи команд.
    pub fn record_prepare_for_render(
        &self,
        command_buffer: vk::CommandBuffer,
        previous: ColorTargetPreviousUse,
        content: ContentOnPrepare,
    ) -> Result<(), GpuError> {
        if previous == ColorTargetPreviousUse::NotUsedYet && content == ContentOnPrepare::Preserve {
            return Err(GpuError::InvalidArgument {
                what: format!("{}: Preserve для цели, которую ещё не использовали", self.label),
            });
        }
        let mut source = previous.source_scope();
        if content == ContentOnPrepare::Discard {
            // Старые пиксели не нужны: переход из UNDEFINED. Ожидание прежних стадий и доступа остаётся.
            source.layout = vk::ImageLayout::UNDEFINED;
        }
        record_image_barrier(
            self.context.device(),
            command_buffer,
            self.image.handle(),
            source,
            render_scope(content),
        );
        Ok(())
    }

    /// Нарисованное отдаёт на чтение сэмплеру стадии `reader` (раскладка `SHADER_READ_ONLY_OPTIMAL`).
    /// Звать после `RasterPass::end`, вне рендеринга.
    pub fn record_render_to_sampled_read(&self, command_buffer: vk::CommandBuffer, reader: ConsumerStage) {
        record_image_barrier(
            self.context.device(),
            command_buffer,
            self.image.handle(),
            ColorTargetPreviousUse::Rendered.source_scope(),
            sampled_read_scope(reader),
        );
    }

    /// Сам `VkImage` (для readback внутри crate).
    pub(crate) fn image_handle(&self) -> vk::Image {
        self.image.handle()
    }

    /// Формат цели в виде перечисления (для декодирования readback).
    pub(crate) fn target_format(&self) -> ColorTargetFormat {
        self.format
    }

    /// Контекст-владелец (для readback).
    pub(crate) fn context(&self) -> &GpuContext {
        &self.context
    }
}
