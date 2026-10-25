//! Мост между Android sync_fd и Vulkan-семафорами.

use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

use ash::vk;

use super::context::VulkanContext;
use super::resources::FrameResources;
use super::vk_step;

/// Результат попытки импортировать acquire-fence как семафор.
pub enum AcquireImport {
    /// Fence стал ожидаемым семафором; fd теперь принадлежит драйверу.
    Imported(vk::Semaphore),
    /// Импорт не удался; fd остался у нас (для CPU-ожидания), причина — текстом.
    Failed { fence: OwnedFd, reason: String },
}

/// Импорт sync_fd в бинарный семафор (`TEMPORARY`: после одного ожидания семафор
/// возвращается в исходное состояние). При успехе fd переходит во владение драйвера.
pub fn import_acquire_fence(ctx: &VulkanContext, resources: &mut FrameResources<'_>, fence: OwnedFd) -> AcquireImport {
    let Some(loader) = ctx.semaphore_fd_loader.as_ref() else {
        return AcquireImport::Failed {
            fence,
            reason: "нет расширения VK_KHR_external_semaphore_fd".to_owned(),
        };
    };
    // SAFETY: обычное создание бинарного семафора на живом device.
    let created = vk_step(
        unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) },
        "vkCreateSemaphore(acquire)",
    );
    let semaphore = match created {
        Ok(semaphore) => semaphore,
        Err(reason) => return AcquireImport::Failed { fence, reason },
    };
    resources.adopt_semaphore(semaphore);

    let import_info = vk::ImportSemaphoreFdInfoKHR::default()
        .semaphore(semaphore)
        .flags(vk::SemaphoreImportFlags::TEMPORARY)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD)
        .fd(fence.as_raw_fd());
    // SAFETY: семафор не в использовании; fd валиден. Владение fd переходит драйверу
    // только при успехе — поэтому `into_raw_fd` вызывается ниже, после проверки результата.
    match vk_step(
        unsafe { loader.import_semaphore_fd(&import_info) },
        "vkImportSemaphoreFdKHR",
    ) {
        Ok(()) => {
            let _ = fence.into_raw_fd(); // fd теперь принадлежит драйверу: закрывать нельзя
            AcquireImport::Imported(semaphore)
        }
        Err(reason) => AcquireImport::Failed { fence, reason },
    }
}

/// Создаёт семафор, пригодный для экспорта в sync_fd (`ExportSemaphoreCreateInfo`).
pub fn create_exportable_semaphore(
    ctx: &VulkanContext,
    resources: &mut FrameResources<'_>,
) -> Result<vk::Semaphore, String> {
    let mut export_info =
        vk::ExportSemaphoreCreateInfo::default().handle_types(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let create_info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
    // SAFETY: create_info и export_info живут до конца вызова.
    let semaphore = vk_step(
        unsafe { ctx.device.create_semaphore(&create_info, None) },
        "vkCreateSemaphore(export)",
    )?;
    resources.adopt_semaphore(semaphore);
    Ok(semaphore)
}

/// Экспорт sync_fd из семафора, уже поставленного на сигнал в `vkQueueSubmit`.
/// `Ok(None)` — драйвер вернул -1: работа уже завершена, ждать нечего.
pub fn export_release_fence(ctx: &VulkanContext, semaphore: vk::Semaphore) -> Result<Option<OwnedFd>, String> {
    let Some(loader) = ctx.semaphore_fd_loader.as_ref() else {
        return Err("нет расширения VK_KHR_external_semaphore_fd".to_owned());
    };
    let get_info = vk::SemaphoreGetFdInfoKHR::default()
        .semaphore(semaphore)
        .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    // SAFETY: семафор создан с экспортом SYNC_FD и стоит в очереди на сигнал.
    let raw_fd = vk_step(unsafe { loader.get_semaphore_fd(&get_info) }, "vkGetSemaphoreFdKHR")?;
    // SAFETY: при raw_fd >= 0 драйвер отдаёт нам владение новым дескриптором.
    Ok((raw_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(raw_fd) }))
}

/// CPU-ожидание fence (fallback, когда импорт семафора невозможен).
pub fn wait_fence_on_cpu(fence: &OwnedFd, timeout_ms: i32) -> Result<(), String> {
    let mut poll_fd = libc::pollfd {
        fd: fence.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll_fd — валидная структура на стеке, nfds = 1.
    let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    match ready {
        1.. => Ok(()),
        0 => Err(format!("poll(acquire fence): таймаут {timeout_ms} мс")),
        _ => Err(format!("poll(acquire fence): {}", std::io::Error::last_os_error())),
    }
}
