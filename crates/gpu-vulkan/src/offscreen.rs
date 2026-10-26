//! Offscreen-цель (RGBA8 UNORM) и чтение её содержимого на CPU — база паритетных тестов.
//!
//! Цель живёт в памяти GPU; `render` записывает в неё команды (картинка переводится в
//! `COLOR_ATTACHMENT_OPTIMAL` и по завершении в `TRANSFER_SRC_OPTIMAL`), `readback` копирует
//! картинку в host-visible буфер и возвращает [`Rgba8Image`]. Все ожидания GPU — с таймаутом
//! контекста (`GpuContextConfig::submit_wait_timeout`).

use ash::vk;

use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory, OwnedImage, OwnedImageView};
use crate::test_pattern::PatternTarget;

/// Прочитанная картинка: `pixels` — `width * height * 4` байт RGBA, строки без выравнивания.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Байты RGBA, построчно сверху вниз.
    pub pixels: Vec<u8>,
}

impl Rgba8Image {
    /// Пиксель `[r, g, b, a]`; `None` — координаты вне картинки.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        let bytes = self.pixels.get(offset..offset + 4)?;
        Some([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

/// Offscreen-цель `RGBA8_UNORM` с использованием `COLOR_ATTACHMENT | TRANSFER_SRC`.
pub struct OffscreenTarget {
    // Порядок полей: представление → картинка → память (уничтожение в таком порядке).
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
    extent: vk::Extent2D,
    /// Текущая раскладка картинки; меняется только после успешного выполнения на GPU.
    layout: vk::ImageLayout,
    context: GpuContext,
}

impl OffscreenTarget {
    /// Формат цели.
    pub const FORMAT: vk::Format = vk::Format::R8G8B8A8_UNORM;

    /// Создаёт цель. Нулевой размер и размер сверх лимита устройства — `InvalidArgument`.
    pub fn new(context: &GpuContext, width: u32, height: u32) -> Result<Self, GpuError> {
        let limit = context.max_image_dimension_2d();
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(GpuError::InvalidArgument {
                what: format!("размер цели {width}x{height} вне 1..={limit}"),
            });
        }
        let device = context.device();
        let image_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(Self::FORMAT)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — живая локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(unsafe { device.create_image(&image_info, None) }, "vkCreateImage")?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "offscreen-картинка")?;
        // SAFETY: память подходит по требованиям и ещё не привязана; смещение 0 кратно выравниванию.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory",
        )?;

        let view_info = vk::ImageViewCreateInfo::default()
            .image(image.handle())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(Self::FORMAT)
            .subresource_range(color_subresource_range());
        // SAFETY: картинка с привязанной памятью, формат совпадает.
        let view = OwnedImageView::from_raw(
            context,
            run_step(
                unsafe { device.create_image_view(&view_info, None) },
                "vkCreateImageView",
            )?,
        );

        Ok(Self {
            view,
            image,
            _memory: memory,
            extent: vk::Extent2D { width, height },
            layout: vk::ImageLayout::UNDEFINED,
            context: context.clone(),
        })
    }

    /// Размер цели.
    pub fn extent(&self) -> vk::Extent2D {
        self.extent
    }

    /// Рисует в цель: выполняет `record` внутри разовой отправки между переходами раскладки.
    /// Замыкание получает командный буфер и описание цели (для `TestPattern::record`).
    ///
    /// Ошибка замыкания или GPU оставляет учтённую раскладку прежней.
    pub fn render(
        &mut self,
        record: impl FnOnce(vk::CommandBuffer, PatternTarget) -> Result<(), GpuError>,
    ) -> Result<(), GpuError> {
        let device = self.context.device().clone();
        let image = self.image.handle();
        let previous_layout = self.layout;
        let target = PatternTarget {
            view: self.view.handle(),
            extent: self.extent,
        };
        self.context.run_one_shot(|command_buffer| {
            // До рисования: любая прежняя раскладка → цель вывода. Если до этого цель читалась
            // копированием, дожидаемся конца чтения (execution dependency).
            let (source_stage, source_access) = if previous_layout == vk::ImageLayout::TRANSFER_SRC_OPTIMAL {
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ)
            } else {
                (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE)
            };
            image_barrier(
                &device,
                command_buffer,
                image,
                (previous_layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
                (source_stage, source_access),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
            );
            record(command_buffer, target)?;
            // После рисования: запись в цель → чтение копированием.
            image_barrier(
                &device,
                command_buffer,
                image,
                (
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                ),
                (
                    vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                    vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
                ),
                (vk::PipelineStageFlags2::COPY, vk::AccessFlags2::TRANSFER_READ),
            );
            Ok(())
        })?;
        self.layout = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
        Ok(())
    }

    /// Копирует содержимое цели на CPU. Требует, чтобы в цель уже рисовали (`render`).
    pub fn readback(&self) -> Result<Rgba8Image, GpuError> {
        if self.layout != vk::ImageLayout::TRANSFER_SRC_OPTIMAL {
            return Err(GpuError::InvalidState {
                what: "readback до первого render: в цели нет содержимого".to_owned(),
            });
        }
        let device = self.context.device();
        let byte_count = u64::from(self.extent.width) * u64::from(self.extent.height) * 4;

        let buffer_info = vk::BufferCreateInfo::default()
            .size(byte_count)
            .usage(vk::BufferUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; buffer_info — живая локальная структура.
        let buffer = OwnedBuffer::from_raw(
            &self.context,
            run_step(unsafe { device.create_buffer(&buffer_info, None) }, "vkCreateBuffer")?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory =
            self.context
                .allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "readback-буфер")?;
        // SAFETY: память подходит по требованиям и ещё не привязана.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory",
        )?;

        let image = self.image.handle();
        let extent = self.extent;
        let destination = buffer.handle();
        self.context.run_one_shot(|command_buffer| {
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
            // SAFETY: буфер записи в состоянии записи; картинка в TRANSFER_SRC_OPTIMAL (проверено выше);
            // буфер достаточного размера (w*h*4), строки плотные (row_length = 0).
            unsafe {
                device.cmd_copy_image_to_buffer(
                    command_buffer,
                    image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    destination,
                    &[region],
                );
            }
            // Запись копирования должна стать видимой хосту.
            let barrier = vk::BufferMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::HOST)
                .dst_access_mask(vk::AccessFlags2::HOST_READ)
                .buffer(destination)
                .offset(0)
                .size(vk::WHOLE_SIZE);
            let barriers = [barrier];
            let dependency = vk::DependencyInfo::default().buffer_memory_barriers(&barriers);
            // SAFETY: командный буфер в состоянии записи, буфер принадлежит этому устройству.
            unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
            Ok(())
        })?;

        let pixels = read_host_memory(&self.context, &memory, byte_count)?;
        Ok(Rgba8Image {
            width: extent.width,
            height: extent.height,
            pixels,
        })
    }
}

/// Читает `byte_count` байт из host-visible когерентной памяти.
fn read_host_memory(context: &GpuContext, memory: &OwnedDeviceMemory, byte_count: u64) -> Result<Vec<u8>, GpuError> {
    let device = context.device();
    let length = usize::try_from(byte_count).map_err(|_| GpuError::InvalidArgument {
        what: "размер readback не помещается в usize".to_owned(),
    })?;
    // SAFETY: память host-visible, не отображена; диапазон 0..byte_count внутри выделения.
    let mapped = run_step(
        unsafe { device.map_memory(memory.handle(), 0, byte_count, vk::MemoryMapFlags::empty()) },
        "vkMapMemory",
    )?;
    // SAFETY: mapped указывает на length читаемых байт; GPU закончил запись (fence ждали), память когерентна.
    let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), length) }.to_vec();
    // SAFETY: память была отображена выше этим же вызовом.
    unsafe { device.unmap_memory(memory.handle()) };
    Ok(bytes)
}

fn color_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
    }
}

/// Записывает барьер раскладки одной цветовой картинки.
/// Аргументы-пары названы по смыслу: `(было, стало)`, `(стадия, доступ)` источника и приёмника.
fn image_barrier(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    (old_layout, new_layout): (vk::ImageLayout, vk::ImageLayout),
    (source_stage, source_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (destination_stage, destination_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
) {
    let barriers = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(source_stage)
        .src_access_mask(source_access)
        .dst_stage_mask(destination_stage)
        .dst_access_mask(destination_access)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(color_subresource_range())];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии записи; картинка принадлежит этому устройству.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_accessor_is_bounds_checked_and_row_major() {
        let image = Rgba8Image {
            width: 2,
            height: 2,
            pixels: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
        };
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(image.pixel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(image.pixel(0, 1), Some([9, 10, 11, 12]));
        assert_eq!(image.pixel(1, 1), Some([13, 14, 15, 16]));
        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 2), None);
    }
}
