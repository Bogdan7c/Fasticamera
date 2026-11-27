//! Картинка прибора: storage-изображение RGBA16F с ПРЯМОЙ альфой и цветами экрана, её промежуточный слой первого
//! прохода показа (сетка, перенесённая в столбцы картинки, решение 322) и то, что о ней знает владелец (последнее
//! использование для барьера следующей записи). Слой зависит от ширины картинки, поэтому живёт и умирает вместе с ней:
//! смена размера пересоздаёт оба, смена прибора и осей — ни одного.

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::DeviceBuffer;
use gpu_vulkan::storage_image::{
    SampledImageView, StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageGeneration,
    StorageImagePreviousUse, StorageImageUsage,
};
use scope_contract::ScopeImageSize;

use super::error::ScopeError;
use super::parameter_blocks::MAX_PLANES;

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
    /// Промежуточный слой «строка сетки × столбец картинки × слот» (f32), под `resampled_rows_capacity` строк.
    resampled_columns: DeviceBuffer,
    /// Сколько строк сетки вмещает промежуточный слой (наибольшая сетка настроек ёмкости).
    resampled_rows_capacity: u32,
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
    /// Новая картинка размера `size` и её промежуточный слой под `resampled_rows_capacity` строк сетки. Формат всегда
    /// fp16: ему нужна прямая альфа без квантования до 2 бит.
    pub(super) fn new(
        context: &GpuContext,
        size: ScopeImageSize,
        resampled_rows_capacity: u32,
    ) -> Result<Self, ScopeError> {
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
        // Слотов на пиксель не больше плоскостей (наложение рисует все), значение — f32.
        let resampled_bytes =
            u64::from(size.width()) * u64::from(resampled_rows_capacity.max(1)) * MAX_PLANES as u64 * 4;
        let resampled_columns = DeviceBuffer::new(context, resampled_bytes)?;
        Ok(Self {
            storage,
            size,
            resampled_columns,
            resampled_rows_capacity: resampled_rows_capacity.max(1),
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

    /// Промежуточный слой первого прохода показа.
    pub(super) fn resampled_columns(&self) -> &DeviceBuffer {
        &self.resampled_columns
    }

    /// Вмещает ли промежуточный слой сетку из `rows` строк.
    pub(super) fn holds_grid_rows(&self, rows: u32) -> bool {
        rows <= self.resampled_rows_capacity
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
