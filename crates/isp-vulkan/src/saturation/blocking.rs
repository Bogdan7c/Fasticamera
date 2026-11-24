//! Блокирующая одноразовая отправка для подготовки адаптеров (создание пайплайнов, картинок, захват RAW).
//!
//! В цикле кадров стенда её НЕ используют: только в конструкторах, до прогона.

use std::time::Duration;

use ash::vk;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::one_shot_queue::{
    CompletedWork, DrainOutcome, OneShotSubmitter, SubmitError, SubmittedWork, WorkStatus,
};

/// Сколько ждём GPU при подготовке (создание пайплайнов и картинок на телефоне — доли секунды; запас большой).
pub(crate) const PREPARATION_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Завершённая блокирующая работа: токен отправки и доказательство завершения.
pub(crate) struct BlockingRun {
    /// Токен отправленной работы (для `note_submitted`/`mark_written_by`).
    pub(crate) work: SubmittedWork,
    /// Доказательство, что GPU закончил именно эту работу.
    pub(crate) proof: CompletedWork,
}

/// Записывает команды, отправляет и ждёт fence (до [`PREPARATION_WAIT_TIMEOUT`]).
pub(crate) fn run_blocking(
    context: &GpuContext,
    record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
) -> Result<BlockingRun, GpuError> {
    let mut submitter = OneShotSubmitter::new(context, 1)?;
    let work = submitter.submit(Vec::new(), record).map_err(|error| match error {
        SubmitError::Record(error) | SubmitError::Gpu(error) => error,
        SubmitError::CapacityExhausted { .. } => GpuError::InvalidState {
            what: "свежая одноразовая очередь без свободного слота".to_owned(),
        },
    })?;
    match submitter.drain(PREPARATION_WAIT_TIMEOUT)? {
        DrainOutcome::Drained => {}
        DrainOutcome::TimedOut { .. } => {
            return Err(GpuError::Timeout {
                step: "OneShotSubmitter::drain(saturation::run_blocking)",
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
