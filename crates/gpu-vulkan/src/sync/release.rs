//! Экспорт release-fence: семафор, из которого после сабмита достаётся `sync_fd`.
//!
//! Правило спецификации: `vkGetSemaphoreFdKHR` для `SYNC_FD` допустим только после того, как
//! операция сигнала семафора отправлена на выполнение (VUID-VkSemaphoreGetFdInfoKHR-handleType-03254;
//! зависимые сигналы тоже должны быть отправлены). Здесь это правило закреплено типами:
//! [`PendingReleaseSignal`] появляется только как результат успешного сабмита
//! ([`SubmittedWork::take_release_signal`](super::SubmittedWork::take_release_signal)),
//! а [`export`](PendingReleaseSignal::export) потребляет его.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedSemaphore;
use shell_contract::sync_fence::GpuReleaseFence;

/// Бинарный семафор, созданный с правом экспорта в `sync_fd`.
///
/// Инвариант: между сабмитом, который его сигналит, и экспортом семафор «занят» —
/// повторный сабмит сигнала (бинарный семафор нельзя сигналить дважды подряд,
/// VUID-vkQueueSubmit2-semaphore-03868) отвергается как `InvalidState`.
/// Экспорт с `SYNC_FD` переносит payload в дескриптор и возвращает семафор в несигнальное
/// состояние — после него семафор можно использовать снова.
pub struct ExportableSemaphore {
    semaphore: OwnedSemaphore,
    /// `true` между успешным сабмитом сигнала и успешным экспортом.
    signal_pending_export: AtomicBool,
    context: GpuContext,
}

impl ExportableSemaphore {
    /// Создаёт семафор с `ExportSemaphoreCreateInfo(SYNC_FD)`.
    ///
    /// Перед созданием спрашивает у драйвера, умеет ли он экспорт `SYNC_FD`
    /// (VUID-VkExportSemaphoreCreateInfo-handleTypes-01124); нет — `GpuError::Unsupported`.
    pub fn new(context: &GpuContext) -> Result<Self, GpuError> {
        // Заодно проверяем, что расширение включено профилем (иначе ExtensionNotEnabled).
        context.external_semaphore_fd_loader()?;
        let handle_type = vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD;
        let query = vk::PhysicalDeviceExternalSemaphoreInfo::default().handle_type(handle_type);
        let mut properties = vk::ExternalSemaphoreProperties::default();
        // SAFETY: physical_device получен из этого instance; структуры локальные.
        unsafe {
            context.instance().get_physical_device_external_semaphore_properties(
                context.physical_device(),
                &query,
                &mut properties,
            );
        }
        if !properties
            .external_semaphore_features
            .contains(vk::ExternalSemaphoreFeatureFlags::EXPORTABLE)
        {
            return Err(GpuError::Unsupported {
                what: "экспорт семафора в SYNC_FD".to_owned(),
            });
        }

        let mut export_info = vk::ExportSemaphoreCreateInfo::default().handle_types(handle_type);
        let create_info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
        // SAFETY: устройство валидно; цепочка pNext живёт до конца вызова.
        let semaphore = OwnedSemaphore::from_raw(
            context,
            run_step(
                unsafe { context.device().create_semaphore(&create_info, None) },
                "vkCreateSemaphore(export)",
            )?,
        );
        Ok(Self {
            semaphore,
            signal_pending_export: AtomicBool::new(false),
            context: context.clone(),
        })
    }

    /// Дескриптор семафора (для `SubmitInfo2`).
    pub fn handle(&self) -> vk::Semaphore {
        self.semaphore.handle()
    }

    /// Занят ли семафор: сигнал отправлен, но fence ещё не экспортирован.
    pub fn is_awaiting_export(&self) -> bool {
        self.signal_pending_export.load(Ordering::Acquire)
    }

    /// Помечает семафор занятым перед сабмитом сигнала. Ошибка — он уже занят.
    pub(crate) fn reserve_for_signal(&self) -> Result<(), GpuError> {
        self.signal_pending_export
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| GpuError::InvalidState {
                what: "семафор экспорта уже сигналится и ждёт экспорта".to_owned(),
            })
    }

    /// Снимает пометку, если сабмит не состоялся (сигнал не отправлен).
    pub(crate) fn cancel_reservation(&self) {
        self.signal_pending_export.store(false, Ordering::Release);
    }

    /// Выдаёт токен «сигнал отправлен» (только `QueueSubmission::submit` после успеха).
    pub(crate) fn pending_signal(&self) -> PendingReleaseSignal<'_> {
        PendingReleaseSignal { semaphore: self }
    }
}

impl std::fmt::Debug for ExportableSemaphore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExportableSemaphore")
            .field("semaphore", &self.semaphore)
            .field("awaiting_export", &self.is_awaiting_export())
            .finish()
    }
}

/// Сигнал семафора отправлен на GPU, fence можно экспортировать. Потребляется экспортом.
#[must_use = "release-fence нужно экспортировать и отдать NDK, иначе буфер камеры не освободится вовремя"]
#[derive(Debug)]
pub struct PendingReleaseSignal<'semaphore> {
    semaphore: &'semaphore ExportableSemaphore,
}

impl PendingReleaseSignal<'_> {
    /// Экспортирует `sync_fd` (`vkGetSemaphoreFdKHR`, `SYNC_FD`).
    ///
    /// Драйвер может вернуть `-1`: работа уже завершена — результат
    /// [`GpuReleaseFence::already_signaled`]. При ошибке семафор остаётся «занятым»
    /// (состояние payload неизвестно), повторно сигналить его нельзя.
    pub fn export(self) -> Result<GpuReleaseFence, GpuError> {
        let loader = self.semaphore.context.external_semaphore_fd_loader()?;
        let get_info = vk::SemaphoreGetFdInfoKHR::default()
            .semaphore(self.semaphore.handle())
            .handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
        // SAFETY: семафор создан с экспортом SYNC_FD, его сигнал уже отправлен
        // (токен выдаётся только после успешного сабмита, VUID-VkSemaphoreGetFdInfoKHR-handleType-03254).
        let raw_fd = run_step(unsafe { loader.get_semaphore_fd(&get_info) }, "vkGetSemaphoreFdKHR")?;
        self.semaphore.cancel_reservation();
        if raw_fd < 0 {
            return Ok(GpuReleaseFence::already_signaled());
        }
        // SAFETY: при неотрицательном значении драйвер отдал нам владение новым дескриптором.
        Ok(GpuReleaseFence::from_sync_fd(unsafe { OwnedFd::from_raw_fd(raw_fd) }))
    }
}
