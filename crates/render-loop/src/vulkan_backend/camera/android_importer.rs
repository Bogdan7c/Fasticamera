//! Импорт буфера камеры в Vulkan на телефоне: `AHardwareBuffer` без копий (решение 40).
//!
//! Тонкая связка над готовыми частями `gpu-vulkan`: кэш импортов по `AHardwareBuffer_getId`
//! ([`AndroidImportCache`]), раскладка из паспорта кадра и барьеры передачи владения с камерой.
//! Проверить это можно только на телефоне; логика кэша и барьеров покрыта тестами на ПК.

use std::num::NonZeroUsize;

use camera_contract::raw_format::RawFormat;
use gpu_vulkan::android::hardware_buffer::HardwareBufferRef;
use gpu_vulkan::android::import::{ImportedHandles, query_hardware_buffer_properties};
use gpu_vulkan::android::import_cache::AndroidImportCache;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::{BufferAcquire, ImageAcquire, ReleaseTarget};
use gpu_vulkan::hardware_buffer_cache::raw_reference::{RawPixelFormat, resolve_row_stride_bytes};
use gpu_vulkan::hardware_buffer_cache::{ImportCacheCounters, ImportLayout};
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use raw_frame_stream::RawFrameLease;

use super::{CameraFrameImporter, CameraImportError, SubmissionSerial};

/// Импортёр буферов камеры поверх `AndroidImportCache`.
pub struct AndroidCameraImporter {
    context: GpuContext,
    cache: AndroidImportCache,
    /// Ёмкость кэша (нужна, чтобы в `Drop` подменить кэш пустым, если старый приходится оставить в утечку).
    capacity: NonZeroUsize,
    /// Последняя серия сабмитов, о завершении которой GPU сообщил (`retire_completed`).
    completed: SubmissionSerial,
}

impl AndroidCameraImporter {
    /// Импортёр с кэшем на `capacity` живых импортов.
    pub fn new(context: &GpuContext, capacity: NonZeroUsize) -> Self {
        Self {
            context: context.clone(),
            cache: AndroidImportCache::new(context, capacity),
            capacity,
            completed: SubmissionSerial(0),
        }
    }

    /// Раскладка импорта по паспорту; для RAW10 шаг строки выводится из описания AHB и размера аллокации.
    fn layout_for(&self, lease: &RawFrameLease, buffer: &HardwareBufferRef) -> Result<ImportLayout, CameraImportError> {
        let stream = &lease.passport().stream;
        let (width, height) = (stream.buffer_size.width, stream.buffer_size.height);
        match stream.raw_format {
            RawFormat::Raw16 => Ok(ImportLayout::SampledImageR16Uint { width, height }),
            RawFormat::Raw10 => {
                let properties = query_hardware_buffer_properties(&self.context, buffer)
                    .map_err(|error| CameraImportError::Platform(error.to_string()))?;
                let row_stride_bytes = resolve_row_stride_bytes(
                    RawPixelFormat::Raw10,
                    width,
                    height,
                    buffer.describe().stride,
                    properties.allocation_size,
                )
                .map_err(|error| CameraImportError::Layout(error.to_string()))?;
                let row_stride_bytes = u32::try_from(row_stride_bytes).map_err(|_| {
                    CameraImportError::Layout(format!("шаг строки {row_stride_bytes} не помещается в u32"))
                })?;
                Ok(ImportLayout::PackedBuffer {
                    row_stride_bytes,
                    rows: height,
                })
            }
        }
    }
}

impl CameraFrameImporter for AndroidCameraImporter {
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError> {
        // Своя ссылка NDK на буфер: заимствование лизинга доказывает, что он жив прямо сейчас.
        let buffer = HardwareBufferRef::from_borrowed_buffer(lease.hardware_buffer())
            .map_err(|error| CameraImportError::Platform(error.to_string()))?;
        let layout = self.layout_for(lease, &buffer)?;
        let stream = &lease.passport().stream;
        let (width, height) = (stream.buffer_size.width, stream.buffer_size.height);
        let acquired = self
            .cache
            .acquire(&buffer, layout, submission)
            .map_err(|error| CameraImportError::Platform(error.to_string()))?;
        Ok(match (acquired.import.handles(), layout) {
            (ImportedHandles::SampledImage { view, image }, _) => MosaicSource::SampledR16 {
                view,
                image,
                width,
                height,
                // Камера — внешний владелец: забираем перед чтением, возвращаем после.
                acquire: ImageAcquire::FromForeignQueue,
                release: ReleaseTarget::ForeignQueue,
            },
            (ImportedHandles::PackedBuffer { buffer, size }, ImportLayout::PackedBuffer { row_stride_bytes, .. }) => {
                MosaicSource::PackedRaw10 {
                    buffer,
                    size_bytes: size,
                    row_stride_bytes,
                    width,
                    height,
                    acquire: BufferAcquire::FromForeignQueue,
                    release: ReleaseTarget::ForeignQueue,
                }
            }
            (ImportedHandles::PackedBuffer { .. }, ImportLayout::SampledImageR16Uint { .. }) => {
                return Err(CameraImportError::Layout(
                    "кэш вернул буфер для раскладки «картинка R16»".to_owned(),
                ));
            }
        })
    }

    fn retire_completed(&mut self, completed: SubmissionSerial) {
        self.completed = self.completed.max(completed);
        self.cache.retire_completed(completed);
    }

    fn release_all_imports_gpu_idle(&mut self) {
        // GPU простаивает (контракт метода): можно объявить завершёнными все возможные серии.
        if let Err(error) = self.cache.clear(SubmissionSerial(u64::MAX)) {
            tracing::error!(target: "fasticamera::render::camera", ?error, "кэш импортов не очищен");
        }
    }

    fn imports_alive(&self) -> usize {
        self.cache.len()
    }

    fn cache_counters(&self) -> ImportCacheCounters {
        self.cache.counters()
    }
}

impl Drop for AndroidCameraImporter {
    /// Уничтожение импортёра НЕ ждёт GPU (поток рендера не ждёт, инвариант 4): импорты серий, о завершении
    /// которых GPU уже сообщил (`retire_completed`), освобождаются сразу, а пустой кэш не вызывает
    /// `vkDeviceWaitIdle`. Если GPU мог ещё читать какой-то импорт (серия не подтверждена), импорты остаются в
    /// учтённой утечке: лучше утечка, чем уничтожение под работающим GPU (соглашение crate).
    fn drop(&mut self) {
        if let Err(error) = self.cache.clear(self.completed) {
            tracing::error!(
                target: "fasticamera::render::camera",
                ?error,
                "импорты снимков, возможно, ещё читает GPU: кэш оставлен в учтённую утечку"
            );
            // Подменяем кэш пустым и забываем старый: иначе его `Drop` ждал бы простоя GPU (`vkDeviceWaitIdle`).
            let stale = std::mem::replace(&mut self.cache, AndroidImportCache::new(&self.context, self.capacity));
            std::mem::forget(stale);
        }
        // Кэш пуст: его `Drop` не ждёт GPU.
    }
}
