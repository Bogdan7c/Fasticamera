//! Ожидание release-fence на CPU без `libc`.
//!
//! В `std` нет `poll(2)`, а новых зависимостей ради него не берём. Вместо этого дескриптор
//! дублируется (`dup` через `OwnedFd::try_clone`) и импортируется во временный `VkFence`
//! (`VK_KHR_external_fence_fd`), которого ждёт обычный `vkWaitForFences`. Исходный fence остаётся
//! у вызывающего нетронутым: его можно ещё отдать NDK.

use std::os::fd::IntoRawFd;
use std::time::Duration;

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedFence;
use shell_contract::sync_fence::GpuReleaseFence;

/// Итог ожидания на CPU. Таймаут — штатный исход, а не ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceWait {
    /// Fence просигналил (или был сигнален сразу).
    Signaled,
    /// За отведённое время сигнала не было.
    TimedOut,
}

// Единое преобразование срока в наносекунды Vulkan: с насыщением на `i64::MAX`, а не на `u64::MAX`
// (последнее у Vulkan значит «ждать вечно», чего в crate быть не должно).
pub(crate) use crate::frame_ring::timeout_nanos;

/// Ждёт сигнала release-fence на CPU не дольше `timeout`.
///
/// Нужно расширение `VK_KHR_external_fence_fd`; без него — `GpuError::ExtensionNotEnabled`.
/// Дескриптор, не являющийся sync-файлом, драйвер отвергнет (`Vulkan { .. }`), fence не пострадает.
pub fn wait_release_fence_on_cpu(
    context: &GpuContext,
    fence: &GpuReleaseFence,
    timeout: Duration,
) -> Result<FenceWait, GpuError> {
    let Some(borrowed_fd) = fence.as_sync_fd() else {
        return Ok(FenceWait::Signaled);
    };
    let loader = context.external_fence_fd_loader()?;
    let duplicate = borrowed_fd
        .try_clone_to_owned()
        .map_err(|error| GpuError::InvalidState {
            what: format!("не удалось продублировать sync_fd: {error}"),
        })?;

    // SAFETY: устройство валидно; create_info — локальная структура.
    let vk_fence = OwnedFence::from_raw(
        context,
        run_step(
            unsafe { context.device().create_fence(&vk::FenceCreateInfo::default(), None) },
            "vkCreateFence(cpu_wait)",
        )?,
    );
    let import_info = vk::ImportFenceFdInfoKHR::default()
        .fence(vk_fence.handle())
        // TEMPORARY обязателен для SYNC_FD (VUID-VkImportFenceFdInfoKHR-handleType-07306).
        .flags(vk::FenceImportFlags::TEMPORARY)
        .handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD)
        .fd(std::os::fd::AsRawFd::as_raw_fd(&duplicate));
    // SAFETY: fence новый и не в использовании; fd открыт. Владение fd уходит драйверу только
    // при успехе, поэтому `into_raw_fd` стоит после проверки результата.
    run_step(unsafe { loader.import_fence_fd(&import_info) }, "vkImportFenceFdKHR")?;
    let _driver_owned_fd = duplicate.into_raw_fd();

    // SAFETY: fence принадлежит этому устройству.
    match unsafe {
        context
            .device()
            .wait_for_fences(&[vk_fence.handle()], true, timeout_nanos(timeout))
    } {
        Ok(()) => Ok(FenceWait::Signaled),
        Err(vk::Result::TIMEOUT) => Ok(FenceWait::TimedOut),
        Err(result) => Err(GpuError::Vulkan {
            step: "vkWaitForFences(cpu_wait)",
            result,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duration_turns_into_the_vulkan_infinite_wait() {
        assert_eq!(timeout_nanos(Duration::from_millis(7)), 7_000_000);
        // u64::MAX у Vulkan — «ждать вечно»; даже заведомо огромный срок не должен им становиться.
        assert_ne!(timeout_nanos(Duration::MAX), u64::MAX);
        assert_ne!(timeout_nanos(Duration::from_secs(u64::MAX / 4)), u64::MAX);
    }
}
