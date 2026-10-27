//! Синтетические RAW-источники на обычной памяти Vulkan — для ПК-тестов предпросмотра и стенда
//! render-loop (там, где нет `AHardwareBuffer`). Доступны под `cfg(test)` и feature `test-support`.
//!
//! Эти ресурсы НЕ из камеры: владение всегда у нашей очереди (`KeepOwned`), раскладка картинки —
//! `SHADER_READ_ONLY_OPTIMAL`. Тем не менее шейдер, дескрипторы и барьеры предпросмотра те же,
//! что на телефоне; на телефоне остаётся проверить только импорт `AHardwareBuffer`.

use ash::vk;

use super::params::MosaicSource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory, OwnedImage, OwnedImageView};

/// Картинка `R16_UINT` с загруженными значениями (RAW16).
pub struct SyntheticRaw16 {
    // Порядок полей = порядок уничтожения: представление → картинка → память.
    view: OwnedImageView,
    image: OwnedImage,
    _memory: OwnedDeviceMemory,
    width: u32,
    height: u32,
}

impl SyntheticRaw16 {
    /// Создаёт картинку `width × height` и загружает `texels` (по строкам). Блокируется до конца копирования.
    pub fn new(context: &GpuContext, width: u32, height: u32, texels: &[u16]) -> Result<Self, GpuError> {
        if texels.len() != width as usize * height as usize || texels.is_empty() {
            return Err(GpuError::InvalidArgument {
                what: format!("{} значений для кадра {width}x{height}", texels.len()),
            });
        }
        let device = context.device();
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
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: устройство валидно; image_info — локальная структура.
        let image = OwnedImage::from_raw(
            context,
            run_step(
                unsafe { device.create_image(&image_info, None) },
                "vkCreateImage(синтетический RAW16)",
            )?,
        );
        // SAFETY: картинка только что создана этим устройством.
        let requirements = unsafe { device.get_image_memory_requirements(image.handle()) };
        let memory = context.allocate_memory(requirements, MemoryLocation::DeviceLocal, "синтетический RAW16")?;
        // SAFETY: память подобрана по требованиям картинки, смещение 0.
        run_step(
            unsafe { device.bind_image_memory(image.handle(), memory.handle(), 0) },
            "vkBindImageMemory(синтетический RAW16)",
        )?;
        let subresource_range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let view_info = vk::ImageViewCreateInfo::default()
            .image(image.handle())
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(vk::Format::R16_UINT)
            .subresource_range(subresource_range);
        // SAFETY: картинка с привязанной памятью.
        let view = OwnedImageView::from_raw(
            context,
            run_step(
                unsafe { device.create_image_view(&view_info, None) },
                "vkCreateImageView(синтетический RAW16)",
            )?,
        );

        let texel_bytes: Vec<u8> = texels.iter().flat_map(|texel| texel.to_le_bytes()).collect();
        let staging = HostVisibleBuffer::with_bytes(context, &texel_bytes, vk::BufferUsageFlags::TRANSFER_SRC)?;
        context.run_one_shot(|command_buffer| {
            let barrier_to_copy = |old_layout, new_layout, src_stage, src_access, dst_stage, dst_access| {
                vk::ImageMemoryBarrier2::default()
                    .src_stage_mask(src_stage)
                    .src_access_mask(src_access)
                    .dst_stage_mask(dst_stage)
                    .dst_access_mask(dst_access)
                    .old_layout(old_layout)
                    .new_layout(new_layout)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(image.handle())
                    .subresource_range(subresource_range)
            };
            let to_copy = [barrier_to_copy(
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::PipelineStageFlags2::NONE,
                vk::AccessFlags2::NONE,
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_WRITE,
            )];
            let to_read = [barrier_to_copy(
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::PipelineStageFlags2::COPY,
                vk::AccessFlags2::TRANSFER_WRITE,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::SHADER_SAMPLED_READ,
            )];
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                });
            // SAFETY: командный буфер в состоянии записи (контракт run_one_shot).
            unsafe {
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_copy),
                );
                device.cmd_copy_buffer_to_image(
                    command_buffer,
                    staging.buffer.handle(),
                    image.handle(),
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
                device.cmd_pipeline_barrier2(
                    command_buffer,
                    &vk::DependencyInfo::default().image_memory_barriers(&to_read),
                );
            }
            Ok(())
        })?;
        Ok(Self {
            view,
            image,
            _memory: memory,
            width,
            height,
        })
    }

    /// Источник для `RawMosaicPreview::record`: картинка наша, раскладка `SHADER_READ_ONLY_OPTIMAL`.
    pub fn source(&self) -> MosaicSource {
        MosaicSource::SampledR16 {
            view: self.view.handle(),
            image: self.image.handle(),
            width: self.width,
            height: self.height,
            acquire: ImageAcquire::FromLayout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
            release: ReleaseTarget::KeepOwned,
        }
    }
}

/// Буфер со словами RAW10 в host-visible памяти.
pub struct SyntheticRaw10 {
    buffer: HostVisibleBuffer,
    row_stride_bytes: u32,
    width: u32,
    height: u32,
}

impl SyntheticRaw10 {
    /// Создаёт буфер из готовых байт RAW10 (`bytes` = `height` строк по `row_stride_bytes`).
    pub fn new(
        context: &GpuContext,
        width: u32,
        height: u32,
        row_stride_bytes: u32,
        bytes: &[u8],
    ) -> Result<Self, GpuError> {
        if bytes.len() as u64 != u64::from(row_stride_bytes) * u64::from(height) || bytes.is_empty() {
            return Err(GpuError::InvalidArgument {
                what: format!("{} байт для {height} строк со шагом {row_stride_bytes}", bytes.len()),
            });
        }
        let buffer = HostVisibleBuffer::with_bytes(context, bytes, vk::BufferUsageFlags::STORAGE_BUFFER)?;
        Ok(Self {
            buffer,
            row_stride_bytes,
            width,
            height,
        })
    }

    /// Источник для `RawMosaicPreview::record`: буфер наш, барьера владения не нужно.
    pub fn source(&self) -> MosaicSource {
        MosaicSource::PackedRaw10 {
            buffer: self.buffer.buffer.handle(),
            size_bytes: self.buffer.size_bytes,
            row_stride_bytes: self.row_stride_bytes,
            width: self.width,
            height: self.height,
            acquire: BufferAcquire::AlreadyOwned,
            release: ReleaseTarget::KeepOwned,
        }
    }
}

/// Буфер в host-visible когерентной памяти с копией байт.
struct HostVisibleBuffer {
    // Порядок полей: буфер уничтожается раньше памяти.
    buffer: OwnedBuffer,
    _memory: OwnedDeviceMemory,
    size_bytes: u64,
}

impl HostVisibleBuffer {
    fn with_bytes(context: &GpuContext, bytes: &[u8], usage: vk::BufferUsageFlags) -> Result<Self, GpuError> {
        let device = context.device();
        let size_bytes = bytes.len() as u64;
        let info = vk::BufferCreateInfo::default()
            .size(size_bytes)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; info — локальная структура.
        let buffer = OwnedBuffer::from_raw(
            context,
            run_step(
                unsafe { device.create_buffer(&info, None) },
                "vkCreateBuffer(синтетический)",
            )?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory =
            context.allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "синтетический буфер")?;
        // SAFETY: буфер и память одного устройства; память host-visible, отображаем ровно bytes.len().
        unsafe {
            run_step(
                device.bind_buffer_memory(buffer.handle(), memory.handle(), 0),
                "vkBindBufferMemory(синтетический)",
            )?;
            let address = run_step(
                device.map_memory(memory.handle(), 0, size_bytes, vk::MemoryMapFlags::empty()),
                "vkMapMemory(синтетический)",
            )?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), address.cast::<u8>(), bytes.len());
            device.unmap_memory(memory.handle());
        }
        Ok(Self {
            buffer,
            _memory: memory,
            size_bytes,
        })
    }
}
