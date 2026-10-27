//! Барьеры storage-картинки, выраженные как намерения («готовим к записи в compute», «записанное
//! отдаём на чтение фрагментному шейдеру»), а не как набор масок.
//!
//! # Почему перезапись картинки безопасна без ожидания на CPU (WAR)
//! Типичный кадр видоискателя: сабмит k читает картинку во фрагментном шейдере (компоновщик), сабмит
//! k+1 на ТОЙ ЖЕ очереди снова пишет её в compute. Это опасность «запись после чтения» (WAR). Первая
//! область синхронизации `vkCmdPipelineBarrier2` включает ВСЕ команды, раньше барьера в порядке
//! отправки на этой очереди, в том числе команды прошлых сабмитов. Поэтому барьер с
//! `srcStageMask = FRAGMENT_SHADER` в начале сабмита k+1 не даст compute-записи начаться, пока не
//! закончатся чтения сабмита k. Для WAR достаточно зависимости исполнения: память, которую только
//! читали, «делать доступной» не нужно, поэтому `srcAccessMask = NONE`. Ожидать fence на CPU не нужно.
//! (Для РАЗНЫХ очередей это не так — там нужны семафоры; в crate очередь одна.)

use ash::vk;

use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;

/// Как картинку использовали в последний раз до записи в compute (что надо дождаться).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageImagePreviousUse {
    /// Картинку ещё не использовали: содержимое не определено, ждать нечего.
    NotUsedYet,
    /// Её писал compute-шейдер (например, предыдущий проход в том же кадре): запись после записи.
    ComputeWrite,
    /// Её читали как sampled image на указанной стадии (компоновщик — `Fragment`, стекло/ISP — `Compute`).
    SampledRead(ConsumerStage),
    /// Её копировали (readback, диагностика).
    TransferRead,
}

/// Область «до барьера»: раскладка, стадия и доступ, которые надо дождаться.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceScope {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

impl StorageImagePreviousUse {
    /// Раскладка и что ждать. Чтение → только зависимость исполнения (WAR, см. документацию модуля).
    pub(crate) fn source_scope(self) -> SourceScope {
        match self {
            Self::NotUsedYet => SourceScope {
                layout: vk::ImageLayout::UNDEFINED,
                stage: vk::PipelineStageFlags2::NONE,
                access: vk::AccessFlags2::NONE,
            },
            Self::ComputeWrite => SourceScope {
                layout: vk::ImageLayout::GENERAL,
                stage: vk::PipelineStageFlags2::COMPUTE_SHADER,
                access: vk::AccessFlags2::SHADER_STORAGE_WRITE,
            },
            Self::SampledRead(reader) => SourceScope {
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                stage: reader.pipeline_stage(),
                access: vk::AccessFlags2::NONE,
            },
            Self::TransferRead => SourceScope {
                layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                stage: vk::PipelineStageFlags2::COPY,
                access: vk::AccessFlags2::NONE,
            },
        }
    }
}

/// Все шейдерные стадии, которые в crate читают storage-картинку как sampled image
/// (`ConsumerStage::Fragment` — компоновщик, `ConsumerStage::Compute` — стекло, ISP, тесты).
pub(crate) const ANY_SHADER_READER_STAGES: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::from_raw(
    vk::PipelineStageFlags2::FRAGMENT_SHADER.as_raw() | vk::PipelineStageFlags2::COMPUTE_SHADER.as_raw(),
);

impl StorageImagePreviousUse {
    /// Как [`Self::source_scope`], но чтение ждёт на ВСЕХ шейдерных стадиях-читателях, а не только на
    /// записанной. Нужно владельцу, который публикует картинку нескольким читателям (компоновщик —
    /// фрагмент, подложка стекла — compute через `BackdropSource::record_read_barrier`): он знает лишь
    /// читателя своей записи, остальные читают сами, и WAR-барьер обязан дождаться их всех.
    pub(crate) fn source_scope_after_any_shader_reader(self) -> SourceScope {
        let scope = self.source_scope();
        match self {
            Self::SampledRead(_) => SourceScope {
                stage: ANY_SHADER_READER_STAGES,
                ..scope
            },
            Self::NotUsedYet | Self::ComputeWrite | Self::TransferRead => scope,
        }
    }
}

/// Область «после барьера».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DestinationScope {
    pub layout: vk::ImageLayout,
    pub stage: vk::PipelineStageFlags2,
    pub access: vk::AccessFlags2,
}

/// Запись в compute: раскладка `GENERAL`, доступ на чтение и запись (шейдер может и читать соседей).
pub(crate) const COMPUTE_WRITE: DestinationScope = DestinationScope {
    layout: vk::ImageLayout::GENERAL,
    stage: vk::PipelineStageFlags2::COMPUTE_SHADER,
    access: vk::AccessFlags2::from_raw(
        vk::AccessFlags2::SHADER_STORAGE_WRITE.as_raw() | vk::AccessFlags2::SHADER_STORAGE_READ.as_raw(),
    ),
};

/// Чтение как sampled image на стадии `reader`.
pub(crate) fn sampled_read(reader: ConsumerStage) -> DestinationScope {
    DestinationScope {
        layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        stage: reader.pipeline_stage(),
        access: vk::AccessFlags2::SHADER_SAMPLED_READ,
    }
}

/// Записывает барьер одной цветовой картинки из `source` в `destination`.
pub(crate) fn record_image_barrier(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    source: SourceScope,
    destination: DestinationScope,
) {
    let barriers = [vk::ImageMemoryBarrier2::default()
        .src_stage_mask(source.stage)
        .src_access_mask(source.access)
        .dst_stage_mask(destination.stage)
        .dst_access_mask(destination.access)
        .old_layout(source.layout)
        .new_layout(destination.layout)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .image(image)
        .subresource_range(vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        })];
    let dependency = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: командный буфер в состоянии записи (контракт вызывающего), вне динамического рендеринга;
    // картинка принадлежит этому устройству; barriers живут до конца вызова.
    unsafe { device.cmd_pipeline_barrier2(command_buffer, &dependency) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_need_only_execution_dependency_and_writes_need_memory_dependency() {
        let fragment_read = StorageImagePreviousUse::SampledRead(ConsumerStage::Fragment).source_scope();
        assert_eq!(fragment_read.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
        assert_eq!(fragment_read.access, vk::AccessFlags2::NONE);
        assert_eq!(fragment_read.layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);

        let compute_read = StorageImagePreviousUse::SampledRead(ConsumerStage::Compute).source_scope();
        assert_eq!(compute_read.stage, vk::PipelineStageFlags2::COMPUTE_SHADER);

        let write = StorageImagePreviousUse::ComputeWrite.source_scope();
        assert_eq!(write.access, vk::AccessFlags2::SHADER_STORAGE_WRITE);
        assert_eq!(write.layout, vk::ImageLayout::GENERAL);

        let fresh = StorageImagePreviousUse::NotUsedYet.source_scope();
        assert_eq!(fresh.layout, vk::ImageLayout::UNDEFINED);
        assert_eq!(fresh.stage, vk::PipelineStageFlags2::NONE);
    }

    #[test]
    fn write_after_any_reader_waits_for_fragment_and_compute_reads() {
        for recorded_reader in [ConsumerStage::Fragment, ConsumerStage::Compute] {
            let scope = StorageImagePreviousUse::SampledRead(recorded_reader).source_scope_after_any_shader_reader();
            assert_eq!(
                scope.stage,
                vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER,
                "записан читатель {recorded_reader:?}, но читать могли и другие"
            );
            assert_eq!(
                scope.access,
                vk::AccessFlags2::NONE,
                "WAR: только зависимость исполнения"
            );
            assert_eq!(scope.layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        }
        // Не-чтения не меняются.
        for other in [
            StorageImagePreviousUse::NotUsedYet,
            StorageImagePreviousUse::ComputeWrite,
            StorageImagePreviousUse::TransferRead,
        ] {
            assert_eq!(other.source_scope_after_any_shader_reader(), other.source_scope());
        }
    }

    #[test]
    fn sampled_read_destination_follows_reader_stage() {
        assert_eq!(
            sampled_read(ConsumerStage::Fragment).stage,
            vk::PipelineStageFlags2::FRAGMENT_SHADER
        );
        assert_eq!(
            sampled_read(ConsumerStage::Compute).stage,
            vk::PipelineStageFlags2::COMPUTE_SHADER
        );
        assert_eq!(
            sampled_read(ConsumerStage::Compute).layout,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
        );
        assert_eq!(COMPUTE_WRITE.layout, vk::ImageLayout::GENERAL);
    }
}
