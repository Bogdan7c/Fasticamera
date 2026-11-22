//! Блокирующие помощники для тестов других crate (`isp-vulkan`): разовая отправка с ожиданием,
//! чтение storage-картинки в `[f32; 4]`, чтение host-буфера.
//!
//! Только под `cfg(test)` и фичей `test-support`: на горячем пути ожиданий GPU нет, а здесь они — суть.
//! Все ожидания ограничены таймаутом контекста (`GpuContextConfig::submit_wait_timeout`).

use ash::vk;

use crate::color_target::{ColorTarget, ColorTargetPreviousUse};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::host_buffer::HostBuffer;
use crate::one_shot_queue::{CompletedWork, DrainOutcome, OneShotSubmitter, SubmitError, SubmittedWork, WorkStatus};
use crate::storage_image::{StorageImage, StorageImagePreviousUse};

/// Записывает команды замыканием, отправляет через [`OneShotSubmitter`] и ждёт завершения (ограниченно).
///
/// Возвращает [`CompletedWork`] — доказательство «fence сигнален» для `HostReadbackBuffer::seal` и
/// подтверждений проявки снимка. Таймаут — `GpuError::Timeout` (срок — `submit_wait_timeout` контекста),
/// объекты отправки при этом намеренно утекают (GPU мог их ещё читать).
pub fn run_one_shot_blocking(
    context: &GpuContext,
    record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
) -> Result<CompletedWork, GpuError> {
    run_one_shot_blocking_traced(context, record).map(|run| run.proof)
}

/// Завершённая блокирующая работа: токен отправки (чтобы отметить записанные ею буферы,
/// `HostReadbackBuffer::mark_written_by`) и доказательство завершения (для `seal`).
pub struct BlockingRun {
    /// Токен отправленной работы.
    pub work: SubmittedWork,
    /// Доказательство завершения этой же работы.
    pub proof: CompletedWork,
}

/// То же, что [`run_one_shot_blocking`], но возвращает и токен работы: `buffer.mark_written_by(&run.work)` →
/// `buffer.seal(&run.proof)`.
pub fn run_one_shot_blocking_traced(
    context: &GpuContext,
    record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
) -> Result<BlockingRun, GpuError> {
    let mut submitter = OneShotSubmitter::new(context, 1)?;
    let work = submitter.submit(Vec::new(), record).map_err(|error| match error {
        SubmitError::Record(error) | SubmitError::Gpu(error) => error,
        SubmitError::CapacityExhausted { .. } => GpuError::InvalidState {
            what: "свежая очередь без свободного слота".to_owned(),
        },
    })?;
    match submitter.drain(context.submit_wait_timeout())? {
        DrainOutcome::Drained => {}
        DrainOutcome::TimedOut { .. } => {
            return Err(GpuError::Timeout {
                step: "OneShotSubmitter::drain(run_one_shot_blocking)",
            });
        }
    }
    match submitter.poll(&work) {
        WorkStatus::Completed(proof) => Ok(BlockingRun { work, proof }),
        WorkStatus::Pending => Err(GpuError::InvalidState {
            what: "после drain работа всё ещё Pending".to_owned(),
        }),
        WorkStatus::Failed(failure) => Err(GpuError::InvalidState {
            what: format!("опрос работы не удался: {failure:?}"),
        }),
    }
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

/// Читает цветовую цель построчно сверху вниз в `[r, g, b, a]` (8/10 бит UNORM и fp16 раскодируются).
///
/// `current_use` — последнее использование цели; после чтения она остаётся в той же раскладке
/// (readback не меняет состояние вызывающего). `NotUsedYet` — `InvalidState`.
pub fn read_color_target_rgba(
    target: &ColorTarget,
    current_use: ColorTargetPreviousUse,
) -> Result<Vec<[f32; 4]>, GpuError> {
    crate::color_target::readback::read_color_target_rgba(target, current_use)
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
