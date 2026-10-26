//! Импорт acquire-fence ([`GpuWaitFence`]) в бинарный семафор для ожидания на GPU.

use std::os::fd::{AsRawFd, IntoRawFd};

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedSemaphore;
use shell_contract::sync_fence::GpuWaitFence;

/// Семафор, которого должен дождаться сабмит; результат импорта fence.
///
/// Семафор импортирован как `TEMPORARY`: после одного ожидания он возвращается в исходное
/// состояние, поэтому на каждый кадр нужен новый импорт. Владелец обязан держать
/// `WaitSemaphore` живым, пока сабмит, который его ждёт, не завершился на GPU.
#[derive(Debug)]
pub enum WaitSemaphore {
    /// Fence уже был сигнален: ждать нечего, семафор не создавался.
    NoWait,
    /// Семафор с импортированным payload (`sync_fd` принадлежит драйверу).
    Imported(OwnedSemaphore),
}

impl WaitSemaphore {
    /// Дескриптор для ожидания; `None` — ждать нечего.
    pub fn semaphore(&self) -> Option<vk::Semaphore> {
        match self {
            Self::NoWait => None,
            Self::Imported(semaphore) => Some(semaphore.handle()),
        }
    }
}

/// Импорт не удался; fence возвращён вызывающему вместе с причиной.
///
/// Вызывающий может дождаться fence на CPU (например, `wait`/`poll` по дескриптору) и
/// продолжить без GPU-ожидания.
#[derive(Debug, thiserror::Error)]
#[error("импорт sync_fd в семафор не удался: {reason}")]
pub struct FenceImportError {
    /// Исходный fence, дескриптор по-прежнему принадлежит ему.
    pub fence: GpuWaitFence,
    /// Причина отказа.
    pub reason: GpuError,
}

/// Превращает fence камеры в семафор ожидания.
///
/// * «уже сигнален» → [`WaitSemaphore::NoWait`], семафор не создаётся;
/// * иначе — `vkImportSemaphoreFdKHR` с `SYNC_FD` и `TEMPORARY`
///   (VUID-VkImportSemaphoreFdInfoKHR-handleType-07307: для типов с копирующей семантикой
///   флаг `TEMPORARY` обязателен; VUID-vkImportSemaphoreFdKHR-semaphore-01142: семафор новый,
///   значит не используется).
///
/// Дескриптор переходит драйверу **только при успехе** (спецификация: при ошибке владение не
/// передаётся). При любой ошибке созданный семафор уничтожается, а fence возвращается внутри
/// [`FenceImportError`].
pub fn import_wait_fence(context: &GpuContext, fence: GpuWaitFence) -> Result<WaitSemaphore, FenceImportError> {
    if fence.is_already_signaled() {
        return Ok(WaitSemaphore::NoWait);
    }
    match import_into_new_semaphore(context, &fence) {
        Ok(semaphore) => {
            // Импорт удался: дескриптор теперь принадлежит драйверу, закрывать его нам нельзя.
            let _driver_owned_fd = fence.into_owned_fd().map(IntoRawFd::into_raw_fd);
            Ok(WaitSemaphore::Imported(semaphore))
        }
        Err(reason) => Err(FenceImportError { fence, reason }),
    }
}

/// Создаёт бинарный семафор и импортирует в него дескриптор fence (не забирая владения).
fn import_into_new_semaphore(context: &GpuContext, fence: &GpuWaitFence) -> Result<OwnedSemaphore, GpuError> {
    let loader = context.external_semaphore_fd_loader()?;
    let Some(sync_fd) = fence.as_sync_fd() else {
        return Err(GpuError::InvalidState {
            what: "у несигнального fence нет дескриптора".to_owned(),
        });
    };
    // SAFETY: устройство валидно; create_info — локальная структура.
    let semaphore = OwnedSemaphore::from_raw(
        context,
        run_step(
            unsafe {
                context
                    .device()
                    .create_semaphore(&vk::SemaphoreCreateInfo::default(), None)
            },
            "vkCreateSemaphore(import)",
        )?,
    );
    let import_info = vk::ImportSemaphoreFdInfoKHR::default()
        .semaphore(semaphore.handle())
        .flags(vk::SemaphoreImportFlags::TEMPORARY)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
        .fd(sync_fd.as_raw_fd());
    // SAFETY: семафор только что создан (не в использовании), fd открыт и живёт дольше вызова.
    run_step(
        unsafe { loader.import_semaphore_fd(&import_info) },
        "vkImportSemaphoreFdKHR",
    )?;
    Ok(semaphore)
}
