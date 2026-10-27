//! Блокирующие помощники для тестов других crate (`isp-vulkan`): разовая отправка с ожиданием,
//! чтение storage-картинки в `[f32; 4]`, чтение host-буфера.
//!
//! Только под `cfg(test)` и фичей `test-support`: на горячем пути ожиданий GPU нет, а здесь они — суть.
//! Все ожидания ограничены таймаутом контекста (`GpuContextConfig::submit_wait_timeout`).

use ash::vk;

use crate::context::GpuContext;
use crate::error::GpuError;
use crate::host_buffer::HostBuffer;
use crate::storage_image::{StorageImage, StorageImagePreviousUse};

/// Записывает команды замыканием в новый командный буфер, отправляет и ждёт завершения.
///
/// Таймаут — `GpuError::Timeout`, объекты отправки при этом намеренно утекают (GPU мог их ещё читать).
pub fn run_one_shot_blocking(
    context: &GpuContext,
    record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
) -> Result<(), GpuError> {
    context.run_one_shot(record)
}

/// Читает storage-картинку построчно сверху вниз в `[r, g, b, a]` (10-бит UNORM и fp16 раскодируются).
///
/// `current_use` — последнее использование картинки; после чтения она остаётся в той же раскладке
/// (readback не меняет состояние вызывающего). `NotUsedYet` — `InvalidState`.
pub fn read_storage_image_rgba(
    image: &StorageImage,
    current_use: StorageImagePreviousUse,
) -> Result<Vec<[f32; 4]>, GpuError> {
    crate::storage_image::readback::read_storage_image_rgba(image, current_use)
}

/// Читает весь host-буфер. Контракт тот же, что у `HostBuffer::read_bytes`: запись GPU завершена и
/// сделана видимой хосту.
pub fn read_host_buffer(buffer: &HostBuffer) -> Result<Vec<u8>, GpuError> {
    let length = usize::try_from(buffer.size_bytes()).map_err(|_| GpuError::InvalidArgument {
        what: "размер буфера не помещается в usize".to_owned(),
    })?;
    buffer.read_bytes(0, length)
}

/// Раскодирует IEEE 754 binary16 в f32 (для тестов, которые читают fp16 из буферов).
pub fn half_to_f32(bits: u16) -> f32 {
    crate::storage_image::readback::half_to_f32(bits)
}
