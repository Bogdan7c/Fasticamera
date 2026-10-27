//! [`ViewfinderImage`] — выход ISP видоискателя: storage-картинка с ЗАКОДИРОВАННЫМИ кривой экрана
//! значениями в ПРЯМОЙ (upright) ориентации, плюс то, что о ней знает производитель.
//!
//! Владелец — `ViewfinderIsp`; наружу — только чтение (представление для компоновщика, подложка
//! стекла, readback в тестах). Раскладку картинка помнит сама: после записи кадра она всегда в
//! `SHADER_READ_ONLY_OPTIMAL` для стадии-читателя этого кадра, следующая запись начинается барьером
//! «чтения читателя → запись compute» (WAR).

use ash::vk;
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::storage_image::{SampledImageView, StorageImage, StorageImageFormat, StorageImagePreviousUse};
use gpu_vulkan::swapchain::DisplayColorSpace;

/// Изображение видоискателя.
pub struct ViewfinderImage {
    storage: StorageImage,
    /// Пространство значений последнего записанного кадра.
    color_space: DisplayColorSpace,
    /// Номер последнего записанного кадра.
    frame: BackdropFrameNumber,
    /// Последнее использование в порядке записи команд (для барьера следующей записи).
    current_use: StorageImagePreviousUse,
}

impl std::fmt::Debug for ViewfinderImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ViewfinderImage")
            .field("storage", &self.storage)
            .field("color_space", &self.color_space)
            .field("frame", &self.frame)
            .field("current_use", &self.current_use)
            .finish()
    }
}

impl ViewfinderImage {
    /// Новая картинка, ещё ни разу не записанная.
    pub(crate) fn new(storage: StorageImage, color_space: DisplayColorSpace) -> Self {
        Self {
            storage,
            color_space,
            frame: BackdropFrameNumber(0),
            current_use: StorageImagePreviousUse::NotUsedYet,
        }
    }

    /// Представление для чтения (компоновщик видоискателя на экран, стекло).
    pub fn sampled_view(&self) -> SampledImageView {
        self.storage.sampled_view()
    }

    /// Размер, пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.storage.extent()
    }

    /// Формат картинки (выбран политикой из конфига по возможностям устройства).
    pub fn format(&self) -> StorageImageFormat {
        self.storage.format()
    }

    /// Пространство значений последнего записанного кадра.
    pub fn color_space(&self) -> DisplayColorSpace {
        self.color_space
    }

    /// Номер последнего записанного кадра.
    pub fn frame(&self) -> BackdropFrameNumber {
        self.frame
    }

    /// Последнее использование в порядке записанных команд (нужно readback-у тестов и диагностике).
    pub fn current_use(&self) -> StorageImagePreviousUse {
        self.current_use
    }

    /// Сама storage-картинка (только чтение: readback, публикация подложки).
    pub fn storage_image(&self) -> &StorageImage {
        &self.storage
    }

    /// Записанный кадр: запоминает его номер, пространство и то, кто читает картинку дальше.
    pub(crate) fn mark_written(
        &mut self,
        frame: BackdropFrameNumber,
        color_space: DisplayColorSpace,
        now_used_as: StorageImagePreviousUse,
    ) {
        self.frame = frame;
        self.color_space = color_space;
        self.current_use = now_used_as;
    }
}
