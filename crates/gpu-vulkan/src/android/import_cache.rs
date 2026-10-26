//! Кэш импортов AHB на Android: Vulkan-бэкенд для нейтральной логики [`crate::hardware_buffer_cache`].
//!
//! Политика (LRU, отложенное уничтожение, счётчики) — в нейтральном модуле и проверена на ПК с
//! fake-бэкендом. Здесь только тонкая связка: ключ из `AHardwareBuffer_getId`, импорт через
//! [`ImportedHardwareBuffer`] и страховка при уничтожении кэша.

use std::num::NonZeroUsize;

use super::hardware_buffer::HardwareBufferRef;
use super::import::{HardwareBufferImportError, ImportedHardwareBuffer};
use crate::context::GpuContext;
use crate::hardware_buffer_cache::{
    Acquired, ClearError, HardwareBufferId, HardwareBufferImportCache, ImportBackend, ImportCacheCounters,
    ImportLayout, SubmissionSerial,
};

/// Бэкенд кэша: настоящий импорт в Vulkan.
pub struct VulkanImportBackend {
    context: GpuContext,
}

impl ImportBackend for VulkanImportBackend {
    type Source = HardwareBufferRef;
    type Import = ImportedHardwareBuffer;
    type Error = HardwareBufferImportError;

    fn import(
        &mut self,
        _id: HardwareBufferId,
        source: &HardwareBufferRef,
        layout: ImportLayout,
    ) -> Result<ImportedHardwareBuffer, HardwareBufferImportError> {
        ImportedHardwareBuffer::import(&self.context, source, layout)
    }
}

/// Кэш импортов AHB. При уничтожении сначала ждёт простоя GPU, потом освобождает импорты:
/// даже если владелец забыл `retire_completed`/`clear`, память не освободится под работающим GPU.
pub struct AndroidImportCache {
    // Порядок полей: кэш (импорты) освобождается ДО контекста, но уже после `Drop::drop` с wait_idle.
    cache: HardwareBufferImportCache<VulkanImportBackend>,
    context: GpuContext,
}

impl AndroidImportCache {
    /// Пустой кэш на `capacity` живых импортов.
    pub fn new(context: &GpuContext, capacity: NonZeroUsize) -> Self {
        let backend = VulkanImportBackend {
            context: context.clone(),
        };
        Self {
            cache: HardwareBufferImportCache::new(backend, capacity),
            context: context.clone(),
        }
    }

    /// Импорт буфера для отправки с серией `submission` (см. [`HardwareBufferImportCache::acquire`]).
    pub fn acquire(
        &mut self,
        hardware_buffer: &HardwareBufferRef,
        layout: ImportLayout,
        submission: SubmissionSerial,
    ) -> Result<Acquired<'_, ImportedHardwareBuffer>, HardwareBufferImportError> {
        let id = hardware_buffer.id()?;
        self.cache.acquire(id, hardware_buffer, layout, submission)
    }

    /// GPU закончил отправки до `completed` включительно: уничтожить ставшие ненужными импорты.
    pub fn retire_completed(&mut self, completed: SubmissionSerial) -> usize {
        self.cache.retire_completed(completed)
    }

    /// Уничтожить все импорты (допустимо только после завершения GPU).
    pub fn clear(&mut self, completed: SubmissionSerial) -> Result<usize, ClearError> {
        self.cache.clear(completed)
    }

    /// Счётчики.
    pub fn counters(&self) -> ImportCacheCounters {
        self.cache.counters()
    }

    /// Живых импортов.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Нет живых импортов.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

impl Drop for AndroidImportCache {
    fn drop(&mut self) {
        // Страховка: импорты освободятся сразу после этого метода, значит GPU не должен их читать.
        if let Err(error) = self.context.wait_idle() {
            tracing::error!(target: "fasticamera::gpu", %error, "wait_idle перед освобождением кэша импортов AHB");
        }
    }
}
