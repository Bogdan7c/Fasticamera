//! Чтение цветовой цели на CPU: в `[f32; 4]` (тесты) или «сырыми» байтами (дайджест стенда насыщения).
//!
//! Блокирующая разовая отправка. Раскладка цели после чтения возвращается к той, что была
//! (`current_use`): readback не меняет состояние, которым владеет вызывающий.

use ash::vk;

use super::{
    ColorTarget, ColorTargetFormat, ColorTargetPreviousUse, ImageScope, record_image_barrier, transfer_read_scope,
};
use crate::error::GpuError;
use crate::host_buffer::{HostBuffer, HostBufferUsage};
use crate::storage_image::StorageImageFormat;
use crate::storage_image::readback::decode_texels;

/// Читает все пиксели цели построчно сверху вниз. `current_use` — как цель использовали последним.
/// `NotUsedYet` — `InvalidState`: читать нечего.
pub(crate) fn read_color_target_rgba(
    target: &ColorTarget,
    current_use: ColorTargetPreviousUse,
) -> Result<Vec<[f32; 4]>, GpuError> {
    let bytes = read_color_target_bytes(target, current_use)?;
    Ok(decode_color_texels(target.target_format(), &bytes))
}

/// Читает «сырые» байты цели: плотные строки `extent.width × extent.height` текселей формата цели (без
/// раскодирования в `f32`). Контракт — как у [`read_color_target_rgba`].
pub(crate) fn read_color_target_bytes(
    target: &ColorTarget,
    current_use: ColorTargetPreviousUse,
) -> Result<Vec<u8>, GpuError> {
    let restore = current_use.restore_scope_after_copy()?;
    let context = target.context();
    let extent = target.extent();
    let byte_count =
        u64::from(extent.width) * u64::from(extent.height) * u64::from(target.target_format().bytes_per_texel());
    let staging = HostBuffer::new(context, byte_count, HostBufferUsage::ReadWriteStorage)?;
    let device = context.device();
    context.run_one_shot(|command_buffer| {
        record_image_barrier(
            device,
            command_buffer,
            target.image_handle(),
            current_use.source_scope(),
            transfer_read_scope(),
        );
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
        // SAFETY: командный буфер в состоянии записи; цель в TRANSFER_SRC_OPTIMAL (барьер выше); буфер
        // вмещает w*h*texel байт плотными строками.
        unsafe {
            device.cmd_copy_image_to_buffer(
                command_buffer,
                target.image_handle(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer_handle(),
                &[region],
            );
        }
        if let Some(restore) = restore {
            let copy_done = ImageScope {
                access: vk::AccessFlags2::NONE,
                ..transfer_read_scope()
            };
            record_image_barrier(device, command_buffer, target.image_handle(), copy_done, restore);
        }
        let host_barrier = [vk::BufferMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(staging.buffer_handle())
            .offset(0)
            .size(vk::WHOLE_SIZE)];
        // SAFETY: командный буфер в состоянии записи; буфер принадлежит устройству.
        unsafe {
            device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&host_barrier),
            );
        }
        Ok(())
    })?;
    let length = usize::try_from(byte_count).map_err(|_| GpuError::InvalidArgument {
        what: "размер readback не помещается в usize".to_owned(),
    })?;
    staging.read_bytes(0, length)
}

/// Раскодирует плотные тексели формата цели в `[r, g, b, a]`.
fn decode_color_texels(format: ColorTargetFormat, bytes: &[u8]) -> Vec<[f32; 4]> {
    match format {
        ColorTargetFormat::Rgba8Unorm => bytes
            .chunks_exact(4)
            .map(|chunk| {
                let unorm = |index: usize| f32::from(chunk[index]) / 255.0;
                [unorm(0), unorm(1), unorm(2), unorm(3)]
            })
            .collect(),
        ColorTargetFormat::A2B10G10R10Unorm => decode_texels(StorageImageFormat::A2B10G10R10Unorm, bytes),
        ColorTargetFormat::Rgba16Float => decode_texels(StorageImageFormat::Rgba16Float, bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_texels_are_decoded_in_rgba_order() {
        let decoded = decode_color_texels(ColorTargetFormat::Rgba8Unorm, &[255, 0, 51, 255, 0, 255, 0, 0]);
        assert_eq!(decoded, vec![[1.0, 0.0, 51.0 / 255.0, 1.0], [0.0, 1.0, 0.0, 0.0]]);
    }

    #[test]
    fn ten_bit_and_half_float_targets_reuse_storage_decoding() {
        let word: u32 = 1023 | (3 << 30);
        assert_eq!(
            decode_color_texels(ColorTargetFormat::A2B10G10R10Unorm, &word.to_le_bytes()),
            vec![[1.0, 0.0, 0.0, 1.0]]
        );
        let half_bytes: Vec<u8> = [0x3C00_u16, 0x0000, 0x3800, 0x3C00]
            .iter()
            .flat_map(|half| half.to_le_bytes())
            .collect();
        assert_eq!(
            decode_color_texels(ColorTargetFormat::Rgba16Float, &half_bytes),
            vec![[1.0, 0.0, 0.5, 1.0]]
        );
    }
}
