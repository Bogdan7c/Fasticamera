//! Чтение storage-картинки на CPU в `[f32; 4]` (только тесты и фича `test-support`).
//!
//! Блокирующая операция (разовая отправка с таймаутом контекста): для паритетных тестов ISP, не для
//! горячего пути. Раскладка картинки после чтения возвращается к той, что была (`current_use`), чтобы
//! readback не менял состояние, которым владеет вызывающий.

use ash::vk;

use super::transitions::{COMPUTE_WRITE, DestinationScope, record_image_barrier, sampled_read};
use super::{StorageImage, StorageImageFormat, StorageImagePreviousUse};
use crate::error::GpuError;
use crate::host_buffer::{HostBuffer, HostBufferUsage};

/// Читает все пиксели картинки построчно сверху вниз. `current_use` — как картинку использовали
/// последним (в той же раскладке она и останется). `NotUsedYet` — `InvalidState`: читать нечего.
pub(crate) fn read_storage_image_rgba(
    image: &StorageImage,
    current_use: StorageImagePreviousUse,
) -> Result<Vec<[f32; 4]>, GpuError> {
    let restore = restore_scope(current_use)?;
    let context = image.context();
    let extent = image.extent();
    let texel_bytes = u64::from(image.format().bytes_per_texel());
    let byte_count = u64::from(extent.width) * u64::from(extent.height) * texel_bytes;
    let staging = HostBuffer::new(context, byte_count, HostBufferUsage::ReadWriteStorage)?;
    let device = context.device();
    let to_transfer = DestinationScope {
        layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        stage: vk::PipelineStageFlags2::COPY,
        access: vk::AccessFlags2::TRANSFER_READ,
    };
    context.run_one_shot(|command_buffer| {
        // Прежняя запись (compute) должна стать видимой копированию: для записи берём область
        // с доступом записи, для чтений — только порядок исполнения.
        record_image_barrier(
            device,
            command_buffer,
            image.image_handle(),
            current_use.source_scope(),
            to_transfer,
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
        // SAFETY: командный буфер в состоянии записи; картинка в TRANSFER_SRC_OPTIMAL (барьер выше);
        // буфер вмещает w*h*texel байт плотными строками.
        unsafe {
            device.cmd_copy_image_to_buffer(
                command_buffer,
                image.image_handle(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                staging.buffer_handle(),
                &[region],
            );
        }
        let copy_done = super::transitions::SourceScope {
            layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            stage: vk::PipelineStageFlags2::COPY,
            access: vk::AccessFlags2::NONE,
        };
        if let Some(restore) = restore {
            record_image_barrier(device, command_buffer, image.image_handle(), copy_done, restore);
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
    let bytes = staging.read_bytes(0, length)?;
    Ok(decode_texels(image.format(), &bytes))
}

/// Куда вернуть картинку после копирования; `None` — оставить в `TRANSFER_SRC_OPTIMAL` (она там и была).
fn restore_scope(current_use: StorageImagePreviousUse) -> Result<Option<DestinationScope>, GpuError> {
    match current_use {
        StorageImagePreviousUse::NotUsedYet => Err(GpuError::InvalidState {
            what: "readback storage-картинки до первой записи: содержимое не определено".to_owned(),
        }),
        StorageImagePreviousUse::ComputeWrite => Ok(Some(COMPUTE_WRITE)),
        StorageImagePreviousUse::SampledRead(reader) => Ok(Some(sampled_read(reader))),
        StorageImagePreviousUse::TransferRead => Ok(None),
    }
}

/// Раскодирует плотные тексели формата в `[r, g, b, a]`.
pub(crate) fn decode_texels(format: StorageImageFormat, bytes: &[u8]) -> Vec<[f32; 4]> {
    match format {
        StorageImageFormat::A2B10G10R10Unorm => bytes
            .chunks_exact(4)
            .map(|chunk| decode_a2b10g10r10(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
            .collect(),
        StorageImageFormat::Rgba16Float => bytes
            .chunks_exact(8)
            .map(|chunk| {
                let half = |index: usize| half_to_f32(u16::from_le_bytes([chunk[index * 2], chunk[index * 2 + 1]]));
                [half(0), half(1), half(2), half(3)]
            })
            .collect(),
    }
}

/// `A2B10G10R10_UNORM_PACK32`: R — биты 0..10, G — 10..20, B — 20..30, A — 30..32.
fn decode_a2b10g10r10(word: u32) -> [f32; 4] {
    const TEN_BIT_MAX: f32 = 1023.0;
    const TWO_BIT_MAX: f32 = 3.0;
    let channel = |shift: u32| ((word >> shift) & 0x3FF) as f32 / TEN_BIT_MAX;
    [channel(0), channel(10), channel(20), (word >> 30) as f32 / TWO_BIT_MAX]
}

/// IEEE 754 binary16 → f32 (точно: любое половинное число представимо в f32).
pub(crate) fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0_f32 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let mantissa = f32::from(bits & 0x3FF);
    match exponent {
        // Ноль и денормализованные: mantissa · 2^-24.
        0 => sign * mantissa * 2.0_f32.powi(-24),
        // Бесконечность и NaN.
        31 if mantissa == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        // Нормализованные: (1 + mantissa/1024) · 2^(exponent-15).
        _ => sign * (1.0 + mantissa / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_float_decoding_matches_known_values() {
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x7BFF), 65504.0);
        assert_eq!(half_to_f32(0x0001), 2.0_f32.powi(-24));
        assert_eq!(half_to_f32(0x7C00), f32::INFINITY);
        assert!(half_to_f32(0x7E00).is_nan());
    }

    #[test]
    fn ten_bit_unorm_channels_are_unpacked_in_rgba_order() {
        // R = 1023, G = 0, B = 512, A = 3.
        let word: u32 = 1023 | (512 << 20) | (3 << 30);
        let decoded = decode_texels(StorageImageFormat::A2B10G10R10Unorm, &word.to_le_bytes());
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0][0], 1.0);
        assert_eq!(decoded[0][1], 0.0);
        assert!((decoded[0][2] - 512.0 / 1023.0).abs() < 1e-6);
        assert_eq!(decoded[0][3], 1.0);
    }

    #[test]
    fn half_float_texels_are_decoded_per_channel() {
        let bytes: Vec<u8> = [0x3C00_u16, 0x3800, 0x0000, 0x3C00]
            .iter()
            .flat_map(|half| half.to_le_bytes())
            .collect();
        assert_eq!(
            decode_texels(StorageImageFormat::Rgba16Float, &bytes),
            vec![[1.0, 0.5, 0.0, 1.0]]
        );
    }
}
