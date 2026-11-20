//! [`ViewfinderImage`] — выход ISP видоискателя: storage-картинка с ЗАКОДИРОВАННЫМИ кривой экрана
//! значениями в ПРЯМОЙ (upright) ориентации, плюс то, что о ней знает производитель.
//!
//! Владелец — `ViewfinderIsp`; наружу — только чтение (представление для компоновщика, подложка
//! стекла, readback в тестах). Раскладку картинка помнит сама: после записи кадра она всегда в
//! `SHADER_READ_ONLY_OPTIMAL` для стадии-читателя этого кадра, следующая запись начинается барьером
//! «чтения читателя → запись compute» (WAR).

use ash::vk;
use gpu_vulkan::backdrop::{BackdropFrameNumber, BackdropSource, SubmissionSerial};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::source_window::SourceWindow;
use gpu_vulkan::storage_image::{SampledImageView, StorageImage, StorageImageFormat, StorageImagePreviousUse};
use gpu_vulkan::swapchain::DisplayColorSpace;

/// Изображение видоискателя.
pub struct ViewfinderImage {
    storage: StorageImage,
    /// Размер содержимого последнего записанного кадра: левый верхний прямоугольник картинки. Картинка
    /// может быть больше (ёмкость под зум 1×), остальное — остатки прежних кадров, читать их нельзя.
    content: vk::Extent2D,
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
            .field("content", &self.content)
            .field("color_space", &self.color_space)
            .field("frame", &self.frame)
            .field("current_use", &self.current_use)
            .finish()
    }
}

impl ViewfinderImage {
    /// Новая картинка, ещё ни разу не записанная.
    pub(crate) fn new(storage: StorageImage, color_space: DisplayColorSpace) -> Self {
        let content = storage.extent();
        Self {
            storage,
            content,
            color_space,
            frame: BackdropFrameNumber(0),
            current_use: StorageImagePreviousUse::NotUsedYet,
        }
    }

    /// Представление для чтения (компоновщик видоискателя на экран, стекло).
    pub fn sampled_view(&self) -> SampledImageView {
        self.storage.sampled_view()
    }

    /// Размер самой картинки (ёмкость), пиксели.
    pub fn extent(&self) -> vk::Extent2D {
        self.storage.extent()
    }

    /// Размер содержимого последнего записанного кадра (≤ `extent`): что именно показывать.
    pub fn content_extent(&self) -> vk::Extent2D {
        self.content
    }

    /// Окно чтения: только содержимое последнего кадра, без зеркала и увеличения. Компоновщик и
    /// подложка стекла обязаны читать картинку через него (см. `gpu_vulkan::source_window`).
    pub fn content_window(&self) -> SourceWindow {
        SourceWindow::of_content(self.content)
    }

    /// Подложка стекла из этой картинки с окном её содержимого; `written_by` — номер сабмита, в который
    /// вызывающий отправил запись (для удержанной картинки — сабмит, записавший её последний кадр).
    ///
    /// Ошибка — только при нарушенном инварианте «содержимое помещается в картинку»: его держат `ensure_image`
    /// и проверка геометрии, так что в работе её не бывает; молча подменять окно на «всю картинку» нельзя
    /// (на стекло попали бы остатки прежних кадров), поэтому ошибка возвращается.
    pub fn backdrop_source(&self, written_by: SubmissionSerial) -> Result<BackdropSource, GpuError> {
        BackdropSource::publish(&self.storage, self.color_space, self.frame, written_by)
            .with_window(self.content_window())
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
        content: vk::Extent2D,
        now_used_as: StorageImagePreviousUse,
    ) {
        self.content = content;
        self.frame = frame;
        self.color_space = color_space;
        self.current_use = now_used_as;
    }
}
