//! Переходы `ColorTarget` как намерения: «готовим к рисованию», «отдаём на чтение сэмплеру».
//!
//! Раскладку цель не отслеживает (барьеры пишутся в командные буферы, которые исполнятся позже):
//! «что было до» сообщает вызывающий через [`ColorTargetPreviousUse`] — он знает порядок своих проходов.

use ash::vk;

use super::barriers::ImageScope;
#[cfg(any(test, feature = "test-support"))]
use crate::error::GpuError;
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;

/// Как цель использовали последним (что надо дождаться и из какой раскладки переходить).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTargetPreviousUse {
    /// Цель ещё не использовали: содержимое не определено, ждать нечего.
    NotUsedYet,
    /// В неё рисовали (проход `RasterPass` или композитор): запись после записи.
    Rendered,
    /// Её читали как sampled image на указанной стадии (пирамида размытия — `Fragment`).
    SampledRead(ConsumerStage),
    /// Её копировали (readback, диагностика).
    TransferRead,
}

/// Что делать с содержимым цели при подготовке к рисованию.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentOnPrepare {
    /// Прежнее содержимое не нужно (проход начнётся с `Clear` или перекроет всё): раскладка берётся
    /// из `UNDEFINED`, драйвер вправе не хранить старые пиксели.
    Discard,
    /// Прежнее содержимое нужно (проход с `AttachmentLoad::Preserve`, смешивание с тем, что было).
    Preserve,
}

impl ColorTargetPreviousUse {
    /// Область «до»: раскладка и что ждать. Чтение → только зависимость исполнения (WAR).
    pub(crate) fn source_scope(self) -> ImageScope {
        match self {
            Self::NotUsedYet => ImageScope {
                layout: vk::ImageLayout::UNDEFINED,
                stage: vk::PipelineStageFlags2::NONE,
                access: vk::AccessFlags2::NONE,
            },
            Self::Rendered => ImageScope {
                layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                access: vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            },
            Self::SampledRead(reader) => ImageScope {
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                stage: reader.pipeline_stage(),
                access: vk::AccessFlags2::NONE,
            },
            Self::TransferRead => ImageScope {
                layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                stage: vk::PipelineStageFlags2::COPY,
                access: vk::AccessFlags2::NONE,
            },
        }
    }

    /// Область, в которую `readback` возвращает картинку после копирования; `None` — она и так в
    /// `TRANSFER_SRC_OPTIMAL`. `NotUsedYet` — `InvalidState`: читать нечего.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn restore_scope_after_copy(self) -> Result<Option<ImageScope>, GpuError> {
        match self {
            Self::NotUsedYet => Err(GpuError::InvalidState {
                what: "readback цели до первой записи: содержимое не определено".to_owned(),
            }),
            Self::Rendered => Ok(Some(render_scope(ContentOnPrepare::Preserve))),
            Self::SampledRead(reader) => Ok(Some(sampled_read_scope(reader))),
            Self::TransferRead => Ok(None),
        }
    }
}

/// Область «после» для рисования. `Preserve` добавляет доступ на чтение: `loadOp = LOAD` и смешивание
/// читают цвет цели на стадии `COLOR_ATTACHMENT_OUTPUT`.
pub(crate) fn render_scope(content: ContentOnPrepare) -> ImageScope {
    let access = match content {
        ContentOnPrepare::Discard => vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        ContentOnPrepare::Preserve => {
            vk::AccessFlags2::COLOR_ATTACHMENT_WRITE | vk::AccessFlags2::COLOR_ATTACHMENT_READ
        }
    };
    ImageScope {
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        stage: vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
        access,
    }
}

/// Область «после» для чтения сэмплером на стадии `reader`.
pub(crate) fn sampled_read_scope(reader: ConsumerStage) -> ImageScope {
    ImageScope {
        layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        stage: reader.pipeline_stage(),
        access: vk::AccessFlags2::SHADER_SAMPLED_READ,
    }
}

/// Область «после» для копирования в буфер (только readback тестов).
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn transfer_read_scope() -> ImageScope {
    ImageScope {
        layout: vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        stage: vk::PipelineStageFlags2::COPY,
        access: vk::AccessFlags2::TRANSFER_READ,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_source_makes_writes_available_and_reads_need_only_execution_dependency() {
        let rendered = ColorTargetPreviousUse::Rendered.source_scope();
        assert_eq!(rendered.access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(rendered.layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        let fragment_read = ColorTargetPreviousUse::SampledRead(ConsumerStage::Fragment).source_scope();
        assert_eq!(fragment_read.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
        assert_eq!(fragment_read.access, vk::AccessFlags2::NONE);
        let fresh = ColorTargetPreviousUse::NotUsedYet.source_scope();
        assert_eq!(fresh.layout, vk::ImageLayout::UNDEFINED);
    }

    #[test]
    fn sampled_read_scope_makes_rendered_writes_visible_to_the_reader_stage() {
        let rendered = ColorTargetPreviousUse::Rendered.source_scope();
        let fragment = sampled_read_scope(ConsumerStage::Fragment);
        assert_eq!(rendered.access, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE);
        assert_eq!(fragment.layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(fragment.stage, vk::PipelineStageFlags2::FRAGMENT_SHADER);
        assert_eq!(fragment.access, vk::AccessFlags2::SHADER_SAMPLED_READ);
        assert_eq!(
            sampled_read_scope(ConsumerStage::Compute).stage,
            vk::PipelineStageFlags2::COMPUTE_SHADER
        );
    }

    #[test]
    fn preserve_adds_read_access_and_discard_does_not() {
        assert!(
            render_scope(ContentOnPrepare::Preserve)
                .access
                .contains(vk::AccessFlags2::COLOR_ATTACHMENT_READ)
        );
        assert!(
            !render_scope(ContentOnPrepare::Discard)
                .access
                .contains(vk::AccessFlags2::COLOR_ATTACHMENT_READ)
        );
    }

    #[test]
    fn readback_restore_returns_to_the_layout_the_caller_had() {
        assert!(matches!(
            ColorTargetPreviousUse::NotUsedYet.restore_scope_after_copy(),
            Err(GpuError::InvalidState { .. })
        ));
        assert_eq!(
            ColorTargetPreviousUse::TransferRead.restore_scope_after_copy(),
            Ok(None)
        );
        let restored = ColorTargetPreviousUse::SampledRead(ConsumerStage::Compute)
            .restore_scope_after_copy()
            .unwrap()
            .unwrap();
        assert_eq!(restored.layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        assert_eq!(restored.stage, vk::PipelineStageFlags2::COMPUTE_SHADER);
    }
}
