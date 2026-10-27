//! Виды привязок набора дескрипторов и ресурсы, которые в них можно записать.
//!
//! Чистая логика без Vulkan-вызовов: «подходит ли ресурс к привязке» проверяется здесь, поэтому
//! ошибка вида «записал буфер в привязку картинки» ловится до драйвера и тестируется без GPU.

use ash::vk;

use crate::error::GpuError;

/// Вид привязки (что объявлено в шейдере на этом `binding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindingKind {
    /// `image2D` с квалификатором формата, запись (`imageStore`). Раскладка картинки — `GENERAL`.
    StorageImageWrite,
    /// `utexture2D`/`texture2D` без сэмплера, чтение `texelFetch` (путь RAW16: камера даёт AHB только
    /// с usage `GPU_SAMPLED_IMAGE`). Раскладка — `SHADER_READ_ONLY_OPTIMAL`.
    SampledImage,
    /// `readonly buffer` (путь RAW10: упакованные слова кадра).
    ReadOnlyStorageBuffer,
    /// `buffer` чтение-запись (статистика кадра для AWB).
    ReadWriteStorageBuffer,
    /// `uniform` блок параметров кадра.
    UniformBuffer,
    /// `sampler2D`: картинка + неизменяемый линейный сэмплер пайплайна (края зажаты).
    /// Раскладка — `SHADER_READ_ONLY_OPTIMAL`.
    CombinedImageSampler,
}

impl BindingKind {
    /// Тип дескриптора Vulkan.
    pub(crate) fn descriptor_type(self) -> vk::DescriptorType {
        match self {
            Self::StorageImageWrite => vk::DescriptorType::STORAGE_IMAGE,
            Self::SampledImage => vk::DescriptorType::SAMPLED_IMAGE,
            Self::ReadOnlyStorageBuffer | Self::ReadWriteStorageBuffer => vk::DescriptorType::STORAGE_BUFFER,
            Self::UniformBuffer => vk::DescriptorType::UNIFORM_BUFFER,
            Self::CombinedImageSampler => vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        }
    }
}

/// Ресурс для записи в привязку. Хэндлы «голые»: владелец ресурса обязан пережить сабмиты, которые
/// читают набор (как у `MosaicSource`); получать их стоит у типов-владельцев
/// (`StorageImage::storage_binding`, `HostBuffer::binding`, `MosaicSource::compute_binding`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorResource {
    /// Представление storage-картинки в раскладке `GENERAL` (для [`BindingKind::StorageImageWrite`]).
    StorageImage {
        /// Представление с usage `STORAGE`.
        view: vk::ImageView,
    },
    /// Представление для чтения в раскладке `SHADER_READ_ONLY_OPTIMAL`. Подходит и к
    /// [`BindingKind::SampledImage`], и к [`BindingKind::CombinedImageSampler`] (сэмплер — пайплайна).
    ReadOnlyImage {
        /// Представление с usage `SAMPLED`.
        view: vk::ImageView,
    },
    /// Буфер только для чтения шейдером (для [`BindingKind::ReadOnlyStorageBuffer`]).
    ReadOnlyBuffer {
        /// Буфер с usage `STORAGE_BUFFER`.
        buffer: vk::Buffer,
        /// Сколько байт с начала видно шейдеру (> 0).
        range_bytes: u64,
    },
    /// Буфер, который шейдер может и писать (подходит к обеим storage-привязкам).
    ReadWriteBuffer {
        /// Буфер с usage `STORAGE_BUFFER`.
        buffer: vk::Buffer,
        /// Сколько байт с начала видно шейдеру (> 0).
        range_bytes: u64,
    },
    /// Uniform-буфер (для [`BindingKind::UniformBuffer`]).
    UniformBuffer {
        /// Буфер с usage `UNIFORM_BUFFER`.
        buffer: vk::Buffer,
        /// Сколько байт с начала видно шейдеру (> 0).
        range_bytes: u64,
    },
}

impl DescriptorResource {
    /// Подходит ли ресурс к привязке. Буфер «только чтение» в привязку записи НЕ подходит: шейдер мог бы
    /// писать в кадр камеры, который ему не принадлежит.
    pub fn fits(&self, kind: BindingKind) -> bool {
        matches!(
            (self, kind),
            (Self::StorageImage { .. }, BindingKind::StorageImageWrite)
                | (
                    Self::ReadOnlyImage { .. },
                    BindingKind::SampledImage | BindingKind::CombinedImageSampler
                )
                | (Self::ReadOnlyBuffer { .. }, BindingKind::ReadOnlyStorageBuffer)
                | (
                    Self::ReadWriteBuffer { .. },
                    BindingKind::ReadOnlyStorageBuffer | BindingKind::ReadWriteStorageBuffer
                )
                | (Self::UniformBuffer { .. }, BindingKind::UniformBuffer)
        )
    }

    /// Размер видимого диапазона буфера; `None` — ресурс не буфер.
    pub(crate) fn buffer_range(&self) -> Option<(vk::Buffer, u64)> {
        match *self {
            Self::ReadOnlyBuffer { buffer, range_bytes }
            | Self::ReadWriteBuffer { buffer, range_bytes }
            | Self::UniformBuffer { buffer, range_bytes } => Some((buffer, range_bytes)),
            Self::StorageImage { .. } | Self::ReadOnlyImage { .. } => None,
        }
    }

    /// Представление и раскладка, в которой шейдер его видит; `None` — ресурс не картинка.
    pub(crate) fn image_view_and_layout(&self) -> Option<(vk::ImageView, vk::ImageLayout)> {
        match *self {
            Self::StorageImage { view } => Some((view, vk::ImageLayout::GENERAL)),
            Self::ReadOnlyImage { view } => Some((view, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)),
            Self::ReadOnlyBuffer { .. } | Self::ReadWriteBuffer { .. } | Self::UniformBuffer { .. } => None,
        }
    }
}

/// Лимиты диапазона буфера в дескрипторе (`VkPhysicalDeviceLimits`): `range` в `VkDescriptorBufferInfo`
/// больше лимита — нарушение `VUID-VkWriteDescriptorSet-descriptorType-00332/00333` (шейдер видел бы
/// не весь буфер или драйвер вёл бы себя непредсказуемо). Проверяется до `vkUpdateDescriptorSets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescriptorRangeLimits {
    /// `maxStorageBufferRange`, байт (спецификация гарантирует ≥ 2^27).
    pub max_storage_buffer_range: u64,
    /// `maxUniformBufferRange`, байт (спецификация гарантирует ≥ 16384).
    pub max_uniform_buffer_range: u64,
}

impl DescriptorRangeLimits {
    /// Наибольший диапазон для вида ресурса; `None` — ресурс не буфер.
    fn limit_for(self, resource: &DescriptorResource) -> Option<u64> {
        match resource {
            DescriptorResource::UniformBuffer { .. } => Some(self.max_uniform_buffer_range),
            DescriptorResource::ReadOnlyBuffer { .. } | DescriptorResource::ReadWriteBuffer { .. } => {
                Some(self.max_storage_buffer_range)
            }
            DescriptorResource::StorageImage { .. } | DescriptorResource::ReadOnlyImage { .. } => None,
        }
    }
}

/// Одна запись в набор: какой привязке какой ресурс.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingWrite {
    /// Номер привязки (`binding` в шейдере) = позиция в `ComputePipelineDesc::bindings`.
    pub binding: u32,
    /// Ресурс.
    pub resource: DescriptorResource,
}

/// Проверяет запись против раскладки: номер в пределах, вид подходит, диапазон буфера ненулевой и не
/// больше лимита устройства для этого типа дескриптора.
pub(crate) fn validate_write(
    layout: &[BindingKind],
    write: &BindingWrite,
    limits: DescriptorRangeLimits,
) -> Result<(), GpuError> {
    let invalid = |what: String| GpuError::InvalidArgument { what };
    let kind = layout
        .get(write.binding as usize)
        .copied()
        .ok_or_else(|| invalid(format!("привязка {} вне 0..{}", write.binding, layout.len())))?;
    if !write.resource.fits(kind) {
        return Err(invalid(format!(
            "ресурс {:?} не подходит к привязке {} вида {kind:?}",
            write.resource, write.binding
        )));
    }
    if let Some((_, range_bytes)) = write.resource.buffer_range()
        && range_bytes == 0
    {
        return Err(invalid(format!("нулевой диапазон буфера в привязке {}", write.binding)));
    }
    if let (Some((_, range_bytes)), Some(limit)) = (write.resource.buffer_range(), limits.limit_for(&write.resource))
        && range_bytes > limit
    {
        return Err(invalid(format!(
            "диапазон буфера {range_bytes} байт в привязке {} больше лимита устройства {limit}",
            write.binding
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: [BindingKind; 3] = [
        BindingKind::StorageImageWrite,
        BindingKind::ReadOnlyStorageBuffer,
        BindingKind::ReadWriteStorageBuffer,
    ];

    /// Минимумы спецификации: storage 2^27, uniform 16384.
    const LIMITS: DescriptorRangeLimits = DescriptorRangeLimits {
        max_storage_buffer_range: 1 << 27,
        max_uniform_buffer_range: 16_384,
    };

    fn write(binding: u32, resource: DescriptorResource) -> BindingWrite {
        BindingWrite { binding, resource }
    }

    fn read_only_buffer(range_bytes: u64) -> DescriptorResource {
        DescriptorResource::ReadOnlyBuffer {
            buffer: vk::Buffer::null(),
            range_bytes,
        }
    }

    #[test]
    fn matching_resources_pass_validation() {
        let storage = DescriptorResource::StorageImage {
            view: vk::ImageView::null(),
        };
        let read_write = DescriptorResource::ReadWriteBuffer {
            buffer: vk::Buffer::null(),
            range_bytes: 16,
        };
        assert_eq!(validate_write(&LAYOUT, &write(0, storage), LIMITS), Ok(()));
        assert_eq!(validate_write(&LAYOUT, &write(1, read_only_buffer(4)), LIMITS), Ok(()));
        // Буфер чтения-записи подходит и к привязке «только чтение».
        assert_eq!(validate_write(&LAYOUT, &write(1, read_write), LIMITS), Ok(()));
        assert_eq!(validate_write(&LAYOUT, &write(2, read_write), LIMITS), Ok(()));
    }

    #[test]
    fn read_only_buffer_is_rejected_for_writable_binding() {
        let error = validate_write(&LAYOUT, &write(2, read_only_buffer(4)), LIMITS).unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { ref what } if what.contains("не подходит")));
    }

    #[test]
    fn wrong_kind_out_of_range_binding_and_empty_range_are_errors() {
        let image = DescriptorResource::ReadOnlyImage {
            view: vk::ImageView::null(),
        };
        assert!(
            validate_write(&LAYOUT, &write(0, image), LIMITS).is_err(),
            "картинка чтения в привязку записи"
        );
        assert!(
            validate_write(&LAYOUT, &write(3, read_only_buffer(4)), LIMITS).is_err(),
            "номер вне раскладки"
        );
        assert!(
            validate_write(&LAYOUT, &write(1, read_only_buffer(0)), LIMITS).is_err(),
            "нулевой диапазон"
        );
    }

    #[test]
    fn buffer_ranges_above_device_limits_are_rejected_per_descriptor_type() {
        let storage_at_limit = read_only_buffer(LIMITS.max_storage_buffer_range);
        assert_eq!(validate_write(&LAYOUT, &write(1, storage_at_limit), LIMITS), Ok(()));
        let storage_over = read_only_buffer(LIMITS.max_storage_buffer_range + 1);
        assert!(validate_write(&LAYOUT, &write(1, storage_over), LIMITS).is_err());
        let uniform_layout = [BindingKind::UniformBuffer];
        let uniform = |range_bytes| DescriptorResource::UniformBuffer {
            buffer: vk::Buffer::null(),
            range_bytes,
        };
        assert_eq!(
            validate_write(&uniform_layout, &write(0, uniform(16_384)), LIMITS),
            Ok(())
        );
        // Тот же размер прошёл бы как storage, но uniform-лимит меньше.
        assert!(validate_write(&uniform_layout, &write(0, uniform(16_385)), LIMITS).is_err());
    }

    #[test]
    fn read_only_image_fits_both_sampled_kinds_and_nothing_else() {
        let image = DescriptorResource::ReadOnlyImage {
            view: vk::ImageView::null(),
        };
        assert!(image.fits(BindingKind::SampledImage));
        assert!(image.fits(BindingKind::CombinedImageSampler));
        assert!(!image.fits(BindingKind::StorageImageWrite));
        assert!(!image.fits(BindingKind::UniformBuffer));
    }
}
