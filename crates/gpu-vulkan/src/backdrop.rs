//! [`BackdropSource`] — нейтральное описание «картинки-подложки», которую читает будущий рендер стекла
//! (сессия 11), не зная, что это кадр камеры.
//!
//! Что внутри: представление для чтения (размер, формат), цветовое пространство значений, номер кадра
//! и готовность. Владелец картинки — производитель (ISP); подложка — снимок «что и когда записано».
//!
//! # Готовность на одной очереди
//! Вся работа в crate идёт на ОДНОЙ очереди. Поэтому «готово» = «записано сабмитом с порядковым
//! номером N» ([`SubmissionSerial`]), а потребителю, который пишет команды в более поздний командный
//! буфер той же очереди, не нужны ни fence, ни семафор: достаточно барьера
//! [`BackdropSource::record_read_barrier`] в его командном буфере. Первая область барьера охватывает
//! все более ранние команды очереди в порядке отправки (спецификация `vkCmdPipelineBarrier2`), то есть
//! и compute-запись производителя из сабмита N.
//!
//! # Контракт производителя
//! Подложку публикует тот, кто в сабмите `written_by` записал картинку compute-шейдером и затем
//! перевёл её в `SHADER_READ_ONLY_OPTIMAL` (`StorageImage::record_compute_write_to_sampled_read`).
//! До следующей перезаписи картинка остаётся в этой раскладке; перезапись начинается барьером
//! `record_prepare_for_compute_write_after_any_reader(SampledRead(..))`, который дожидается чтений
//! потребителя на ЛЮБОЙ шейдерной стадии (WAR): производитель не знает, кто ещё читал подложку.
//!
//! # Время жизни картинки и контракт потребителя (стекло, сессия 11)
//! Подложка хранит «голые» хэндлы. Картинка производителя может смениться (пересоздание под новый
//! размер): старая уходит в отложенное уничтожение и освобождается, когда производитель увидит fence
//! более поздней своей записи. Поэтому потребитель:
//! 1. берёт подложку у производителя ЗАНОВО на каждый кадр (`ViewfinderIsp::backdrop_source`) и не
//!    хранит её между кадрами;
//! 2. если всё же держит подложку дольше одного кадра — перед КАЖДОЙ записью чтения спрашивает
//!    производителя, актуальна ли она (`ViewfinderIsp::is_backdrop_current`); устаревшую выбрасывает,
//!    не записав ни одной команды с ней (иначе — чтение уничтоженной картинки);
//! 3. записывает чтения только в сабмиты, отправленные ДО того, как производитель освободит картинку:
//!    на одной очереди это выполнено автоматически, если пункт 2 соблюдён на потоке рендера (запись и
//!    освобождение идут на нём же последовательно).
//!
//! Поколение картинки ([`BackdropSource::image_generation`]) уникально на процесс, поэтому сравнение
//! не обманется переиспользованным драйвером хэндлом.

use ash::vk;

use crate::context::GpuContext;
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;
use crate::storage_image::{SampledImageView, StorageImage, StorageImageGeneration};
use crate::swapchain::DisplayColorSpace;

/// Номер кадра подложки (монотонный у производителя).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BackdropFrameNumber(pub u64);

/// Порядковый номер сабмита на единственной очереди (монотонный у того, кто отправляет).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubmissionSerial(pub u64);

/// Подложка: что читать и когда оно записано.
#[derive(Clone)]
pub struct BackdropSource {
    view: SampledImageView,
    image: vk::Image,
    /// Поколение картинки, из которой опубликована подложка (проверка актуальности у производителя).
    image_generation: StorageImageGeneration,
    color_space: DisplayColorSpace,
    frame: BackdropFrameNumber,
    written_by: SubmissionSerial,
    context: GpuContext,
}

impl std::fmt::Debug for BackdropSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackdropSource")
            .field("view", &self.view)
            .field("image_generation", &self.image_generation)
            .field("color_space", &self.color_space)
            .field("frame", &self.frame)
            .field("written_by", &self.written_by)
            .finish()
    }
}

impl BackdropSource {
    /// Публикует подложку из storage-картинки производителя (см. контракт производителя в модуле).
    ///
    /// Хэндлы «голые»: картинка должна пережить все сабмиты, которые читают подложку (как у `MosaicSource`).
    pub fn publish(
        image: &StorageImage,
        color_space: DisplayColorSpace,
        frame: BackdropFrameNumber,
        written_by: SubmissionSerial,
    ) -> Self {
        Self {
            view: image.sampled_view(),
            image: image.image_handle(),
            image_generation: image.generation(),
            color_space,
            frame,
            written_by,
            context: image.context().clone(),
        }
    }

    /// Представление для чтения: размер и формат.
    pub fn view(&self) -> SampledImageView {
        self.view
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.view.extent
    }

    /// Формат.
    pub fn format(&self) -> vk::Format {
        self.view.format
    }

    /// В каком пространстве закодированы значения (как у выхода на экран).
    pub fn color_space(&self) -> DisplayColorSpace {
        self.color_space
    }

    /// Номер кадра.
    pub fn frame(&self) -> BackdropFrameNumber {
        self.frame
    }

    /// Поколение картинки производителя, из которой опубликована подложка.
    pub fn image_generation(&self) -> StorageImageGeneration {
        self.image_generation
    }

    /// Опубликована ли подложка из картинки `image` (той же самой, а не пересозданной на её месте).
    /// Производитель отвечает этим на вопрос «актуальна ли подложка» (см. контракт потребителя).
    pub fn is_from(&self, image: &StorageImage) -> bool {
        self.image_generation == image.generation()
    }

    /// Каким сабмитом записана картинка.
    pub fn written_by(&self) -> SubmissionSerial {
        self.written_by
    }

    /// Можно ли читать подложку из сабмита `consumer`: только из более позднего, чем записавший
    /// (тот же сабмит читает её своими барьерами производителя, более ранний — ещё до записи).
    pub fn is_ready_for(&self, consumer: SubmissionSerial) -> bool {
        consumer > self.written_by
    }

    /// Барьер в командном буфере потребителя перед чтением на стадии `reader`: compute-запись
    /// производителя → чтение `reader`. Раскладка не меняется (`SHADER_READ_ONLY_OPTIMAL` →
    /// `SHADER_READ_ONLY_OPTIMAL`), поэтому барьер безопасно повторять — он лишь гарантирует видимость.
    /// Записывать вне динамического рендеринга.
    pub fn record_read_barrier(&self, command_buffer: vk::CommandBuffer, reader: ConsumerStage) {
        let barriers = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COMPUTE_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_STORAGE_WRITE)
            .dst_stage_mask(reader.pipeline_stage())
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(self.image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })];
        let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
        // SAFETY: командный буфер в состоянии записи вне рендеринга (контракт метода); картинка жива
        // (контракт `publish`) и в SHADER_READ_ONLY_OPTIMAL (контракт производителя).
        unsafe { self.context.device().cmd_pipeline_barrier2(command_buffer, &dependency) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::storage_image::{StorageFormatPolicy, StorageImageDesc, StorageImageUsage};

    #[test]
    fn readiness_requires_a_later_submission_and_metadata_is_preserved() {
        let Some(context) = gpu_context_or_skip() else { return };
        let image = StorageImage::new(
            &context,
            &StorageImageDesc {
                extent: vk::Extent2D { width: 4, height: 2 },
                format_policy: StorageFormatPolicy::HalfFloatOnly,
                usage: StorageImageUsage::ComputeWriteSampledRead,
            },
        )
        .unwrap();
        let backdrop = BackdropSource::publish(
            &image,
            DisplayColorSpace::DisplayP3,
            BackdropFrameNumber(7),
            SubmissionSerial(10),
        );
        assert!(!backdrop.is_ready_for(SubmissionSerial(9)));
        assert!(
            !backdrop.is_ready_for(SubmissionSerial(10)),
            "тот же сабмит — не потребитель"
        );
        assert!(backdrop.is_ready_for(SubmissionSerial(11)));
        assert_eq!(backdrop.extent(), vk::Extent2D { width: 4, height: 2 });
        assert_eq!(backdrop.format(), vk::Format::R16G16B16A16_SFLOAT);
        assert_eq!(backdrop.color_space(), DisplayColorSpace::DisplayP3);
        assert_eq!(backdrop.frame(), BackdropFrameNumber(7));
        assert_eq!(backdrop.view(), image.sampled_view());
        assert!(backdrop.is_from(&image));
        assert_eq!(backdrop.image_generation(), image.generation());
    }

    #[test]
    fn backdrop_from_a_replaced_image_is_not_from_the_new_one() {
        let Some(context) = gpu_context_or_skip() else { return };
        let desc = StorageImageDesc {
            extent: vk::Extent2D { width: 4, height: 2 },
            format_policy: StorageFormatPolicy::HalfFloatOnly,
            usage: StorageImageUsage::ComputeWriteSampledRead,
        };
        let old_image = StorageImage::new(&context, &desc).unwrap();
        let backdrop = BackdropSource::publish(
            &old_image,
            DisplayColorSpace::Srgb,
            BackdropFrameNumber(1),
            SubmissionSerial(1),
        );
        // Даже если драйвер переиспользует хэндл после уничтожения, поколение у новой картинки другое.
        drop(old_image);
        let new_image = StorageImage::new(&context, &desc).unwrap();
        assert!(!backdrop.is_from(&new_image));
        assert_ne!(backdrop.image_generation(), new_image.generation());
    }
}
