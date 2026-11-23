//! Картинка прибора: storage-изображение RGBA16F с ПРЯМОЙ альфой и цветами экрана, плюс то, что о ней знает
//! владелец (последнее использование для барьера следующей записи).

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::storage_image::{
    SampledImageView, StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageGeneration,
    StorageImagePreviousUse, StorageImageUsage,
};
use scope_contract::ScopeImageSize;

use super::error::ScopeError;

/// Что произошло с картинкой прибора при подготовке или записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureChange {
    /// Действующая картинка нужного размера: ничего не изменилось.
    Unchanged,
    /// Картинки не было, создана первая.
    Created,
    /// Прежняя картинка другого размера заменена (она ждёт завершения GPU перед уничтожением).
    Replaced,
}

/// Картинка прибора.
pub(super) struct PictureTarget {
    storage: StorageImage,
    size: ScopeImageSize,
    /// Последнее использование в порядке записи команд: перед следующей записью ждём чтений.
    current_use: StorageImagePreviousUse,
    /// Начинали ли команды записываться с этой картинкой (см. [`PictureTarget::is_referenced_by_commands`]).
    referenced_by_commands: bool,
}

impl std::fmt::Debug for PictureTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureTarget")
            .field("storage", &self.storage)
            .field("current_use", &self.current_use)
            .finish()
    }
}

impl PictureTarget {
    /// Новая картинка размера `size`. Формат всегда fp16: ему нужна прямая альфа без квантования до 2 бит.
    pub(super) fn new(context: &GpuContext, size: ScopeImageSize) -> Result<Self, ScopeError> {
        let storage = StorageImage::new(
            context,
            &StorageImageDesc {
                extent: vk::Extent2D {
                    width: size.width(),
                    height: size.height(),
                },
                format_policy: StorageFormatPolicy::HalfFloatOnly,
                usage: StorageImageUsage::ComputeWriteSampledRead,
            },
        )?;
        Ok(Self {
            storage,
            size,
            current_use: StorageImagePreviousUse::NotUsedYet,
            referenced_by_commands: false,
        })
    }

    pub(super) fn size(&self) -> ScopeImageSize {
        self.size
    }

    pub(super) fn storage(&self) -> &StorageImage {
        &self.storage
    }

    pub(super) fn generation(&self) -> StorageImageGeneration {
        self.storage.generation()
    }

    pub(super) fn sampled_view(&self) -> SampledImageView {
        self.storage.sampled_view()
    }

    pub(super) fn current_use(&self) -> StorageImagePreviousUse {
        self.current_use
    }

    /// Ссылается ли на картинку хоть одна записанная команда. Пока нет (картинку только подготовили), уничтожить её
    /// можно немедленно (см. `ColorScopes::adopt_picture`).
    pub(super) fn is_referenced_by_commands(&self) -> bool {
        self.referenced_by_commands
    }

    /// Запись кадра начинает писать команды с этой картинкой (отмечается ДО первой команды: ошибка GPU посреди записи
    /// оставляет в буфере частичные команды, и картинку уже нельзя уничтожать немедленно).
    pub(super) fn note_referenced_by_commands(&mut self) {
        self.referenced_by_commands = true;
    }

    /// После записи команд: картинка отдана фрагментному читателю (раскладка `SHADER_READ_ONLY_OPTIMAL`).
    pub(super) fn mark_published_to_fragment_reader(&mut self) {
        self.current_use = StorageImagePreviousUse::SampledRead(
            gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage::Fragment,
        );
    }
}
