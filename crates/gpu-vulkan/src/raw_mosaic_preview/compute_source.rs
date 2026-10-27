//! Кадр камеры ([`MosaicSource`]) как вход compute-шейдера (сессия 07: ISP видоискателя на GPU).
//!
//! Тип источника тот же, что у отладочной мозаики, — меняется только стадия-потребитель. Семантика
//! владения прежняя (`render-loop/src/vulkan_backend/camera/ownership.rs`):
//! * **забрать один раз** — `record_acquire_for` в ПЕРВОМ сабмите, который читает новый кадр;
//!   перерисовка того же кадра барьеров владения не пишет;
//! * **вернуть один раз** — `record_release_for` в сабмите, который ЗАМЕНЯЕТ кадр (его fence потом
//!   экспортируется камере как release-fence и покрывает и сам барьер возврата).
//!
//! Стадия в обоих барьерах должна совпадать со стадией, которая реально читает кадр (`Compute` для
//! ISP): acquire открывает доступ этой стадии, release ждёт её чтений.

use ash::vk;

use super::params::MosaicSource;
use super::{record_acquire, record_release};
use crate::compute::{BindingKind, DescriptorResource};
use crate::context::GpuContext;
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;

impl MosaicSource {
    /// Вид привязки, под который объявлять вход в шейдере: RAW16 — `SampledImage` (`utexture2D` +
    /// `texelFetch`), RAW10 — `ReadOnlyStorageBuffer` (слова упакованного кадра).
    pub fn compute_binding_kind(&self) -> BindingKind {
        match self {
            Self::SampledR16 { .. } => BindingKind::SampledImage,
            Self::PackedRaw10 { .. } => BindingKind::ReadOnlyStorageBuffer,
        }
    }

    /// Ресурс для записи в compute-набор (только чтение: шейдер не может писать в кадр камеры).
    pub fn compute_binding(&self) -> DescriptorResource {
        match *self {
            Self::SampledR16 { view, .. } => DescriptorResource::ReadOnlyImage { view },
            Self::PackedRaw10 { buffer, size_bytes, .. } => DescriptorResource::ReadOnlyBuffer {
                buffer,
                range_bytes: size_bytes,
            },
        }
    }

    /// Записывает приём кадра у камеры перед первым чтением стадией `reader` (по `acquire` источника:
    /// `FOREIGN → наша очередь` для кадра камеры, переход раскладки или ничего — для источников ПК).
    /// Записывать вне динамического рендеринга, один раз на кадр.
    pub fn record_acquire_for(&self, context: &GpuContext, command_buffer: vk::CommandBuffer, reader: ConsumerStage) {
        record_acquire(
            context.device(),
            command_buffer,
            context.queue_family_index(),
            self,
            reader,
        );
    }

    /// Записывает возврат кадра камере после чтений стадией `reader` (по `release` источника:
    /// `наша очередь → FOREIGN` или ничего для `KeepOwned`). Записывать в сабмите, заменяющем кадр.
    pub fn record_release_for(&self, context: &GpuContext, command_buffer: vk::CommandBuffer, reader: ConsumerStage) {
        record_release(
            context.device(),
            command_buffer,
            context.queue_family_index(),
            self,
            reader,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};

    #[test]
    fn camera_sources_bind_read_only_with_matching_kinds() {
        let raw16 = MosaicSource::SampledR16 {
            view: vk::ImageView::null(),
            image: vk::Image::null(),
            width: 4,
            height: 4,
            acquire: ImageAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        };
        assert_eq!(raw16.compute_binding_kind(), BindingKind::SampledImage);
        assert!(raw16.compute_binding().fits(raw16.compute_binding_kind()));

        let raw10 = MosaicSource::PackedRaw10 {
            buffer: vk::Buffer::null(),
            size_bytes: 80,
            row_stride_bytes: 20,
            width: 16,
            height: 4,
            acquire: BufferAcquire::FromForeignQueue,
            release: ReleaseTarget::ForeignQueue,
        };
        assert_eq!(raw10.compute_binding_kind(), BindingKind::ReadOnlyStorageBuffer);
        assert_eq!(
            raw10.compute_binding(),
            DescriptorResource::ReadOnlyBuffer {
                buffer: vk::Buffer::null(),
                range_bytes: 80
            }
        );
        // Кадр камеры нельзя привязать как буфер записи.
        assert!(!raw10.compute_binding().fits(BindingKind::ReadWriteStorageBuffer));
    }
}
