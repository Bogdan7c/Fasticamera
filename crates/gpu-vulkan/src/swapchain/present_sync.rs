//! Дополнительная синхронизация сабмита кадра показа: ожидание кадра камеры и экспорт release-fence.
//!
//! Обычный показ ждёт только «картинка swapchain получена» и сигналит «рендер готов» + fence слота.
//! Кадр камеры добавляет две вещи (сессия 05, решения 31 и 40):
//! * **ждать acquire-fence камеры** перед первым использованием её буфера
//!   ([`PresentSync::wait_for_camera_acquire`]); ожидание всегда на стадии `ALL_COMMANDS` — см. ниже;
//! * **сигналить экспортируемый семафор** ([`PresentSync::export_release_after_submit`]): его fence
//!   вызывающий забирает после сабмита и отдаёт камере как release-fence.
//!
//! # Почему fence позднего сабмита освобождает и ранние кадры
//! Спецификация `vkQueueSubmit2`: «The first synchronization scope of each semaphore signal operation
//! ... includes every command in the same batch ... and all commands that occur earlier in
//! submission order». Значит, fence, экспортированный из сигнала сабмита k, срабатывает не раньше
//! окончания ВСЕХ сабмитов до k на этой очереди. Поэтому кадр камеры, который читал сабмит j < k,
//! можно безопасно вернуть камере с fence сабмита k — именно так render-loop освобождает ПРЕДЫДУЩИЙ
//! кадр fence-ом сабмита, который уже показывает следующий.
//!
//! # Почему ожидание fence камеры — на `ALL_COMMANDS`, а не на стадии шейдера-читателя
//! В том же сабмите, что ждёт fence, записан «забор» буфера у `QUEUE_FAMILY_FOREIGN` (acquire-барьер,
//! для RAW16 — ещё и переход раскладки `UNDEFINED → SHADER_READ_ONLY`). Спецификация
//! (synchronization.adoc, примечание к queue family ownership transfer): «Prior to the introduction of
//! VK_DEPENDENCY_QUEUE_FAMILY_OWNERSHIP_TRANSFER_USE_ALL_STAGES_BIT_KHR, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT
//! is the only valid stage to wait on or wait for these operations, as the acquire and release
//! operations do not occur in a defined stage». Ожидание на стадии фрагмента/compute не упорядочивает
//! сам acquire (и переход раскладки) после записи камерой — он мог бы обогнать камеру. maintenance8 на
//! Adreno 740 (Vulkan 1.3.128) нет, поэтому — `ALL_COMMANDS`. Цена мала: fence камеры почти всегда
//! приходит уже сигнальным (факт 00/05), и тогда ожидания нет вовсе (`WaitSemaphore::NoWait`).
//! Дефект найден ревью сессии 07 (до этого с 05 ожидание стояло на `FRAGMENT_SHADER`).

use crate::swapchain::SwapchainError;
use crate::swapchain::frame::PresentOutcome;
use crate::sync::{ExportableSemaphore, PendingReleaseSignal, WaitSemaphore};

/// Что добавить к сабмиту кадра показа. По умолчанию (`PresentSync::none()`) — ничего:
/// поведение `present` не меняется.
#[derive(Debug, Clone, Copy, Default)]
pub struct PresentSync<'resources> {
    camera_acquire: Option<&'resources WaitSemaphore>,
    release_export: Option<&'resources ExportableSemaphore>,
}

impl<'resources> PresentSync<'resources> {
    /// Без дополнительной синхронизации.
    pub fn none() -> Self {
        Self::default()
    }

    /// Сабмит дождётся импортированного acquire-fence камеры на стадии `ALL_COMMANDS` (почему —
    /// в доке модуля: забор буфера у внешней очереди не имеет своей стадии). Это верно для любого
    /// читателя — фрагментного шейдера (отладочная мозаика) или compute-прохода ISP.
    /// `WaitSemaphore::NoWait` (fence уже сигнален) ничего не добавляет.
    #[must_use]
    pub fn wait_for_camera_acquire(mut self, wait: &'resources WaitSemaphore) -> Self {
        self.camera_acquire = Some(wait);
        self
    }

    /// Сабмит просигналит `semaphore` (стадия `ALL_COMMANDS`); после успешного сабмита fence берётся из
    /// [`SyncedPresent::release_signal`].
    #[must_use]
    pub fn export_release_after_submit(mut self, semaphore: &'resources ExportableSemaphore) -> Self {
        self.release_export = Some(semaphore);
        self
    }

    pub(crate) fn camera_acquire_semaphore(&self) -> Option<ash::vk::Semaphore> {
        self.camera_acquire.and_then(WaitSemaphore::semaphore)
    }

    /// Стадия ожидания acquire-fence камеры в сабмите: всегда `ALL_COMMANDS` (см. доку модуля).
    pub(crate) fn camera_acquire_wait_stage(&self) -> ash::vk::PipelineStageFlags2 {
        ash::vk::PipelineStageFlags2::ALL_COMMANDS
    }

    pub(crate) fn release_export(&self) -> Option<&'resources ExportableSemaphore> {
        self.release_export
    }
}

/// Дошёл ли сабмит кадра до очереди. От этого зависит судьба кадров камеры: после сабмита GPU мог их
/// читать (вернуть можно только по fence), до сабмита — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitProgress {
    /// Сабмит не принят очередью (ошибка до или в `vkQueueSubmit2`): GPU кадр не читал.
    NotSubmitted,
    /// Сабмит принят: GPU прочитает (или уже читает) всё, что в нём записано, даже если показ потом
    /// вернул ошибку.
    Submitted,
}

/// Итог `present_with_sync`: исход показа плюс то, что нужно знать о сабмите независимо от исхода.
#[must_use = "release_signal надо экспортировать, иначе буфер камеры нельзя вернуть по fence"]
#[derive(Debug)]
pub struct SyncedPresent<'resources> {
    /// Дошёл ли сабмит до очереди.
    pub submit: SubmitProgress,
    /// Токен экспорта release-fence: `Some`, только если сабмит принят и экспорт был запрошен.
    pub release_signal: Option<PendingReleaseSignal<'resources>>,
    /// Исход показа (ошибка показа возможна и после принятого сабмита).
    pub outcome: Result<PresentOutcome, SwapchainError>,
}

impl SyncedPresent<'_> {
    /// Ошибка до сабмита: сабмит не состоялся, токена нет.
    pub(crate) fn failed_before_submit(error: impl Into<SwapchainError>) -> Self {
        Self {
            submit: SubmitProgress::NotSubmitted,
            release_signal: None,
            outcome: Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use ash::vk;

    use super::*;

    #[test]
    fn camera_acquire_wait_covers_all_commands_because_foreign_acquire_has_no_stage() {
        let no_wait = WaitSemaphore::NoWait;
        // Ожидание обязано покрывать acquire-барьер внешней очереди: по спецификации это возможно
        // только на ALL_COMMANDS (до maintenance8). Стадия шейдера-читателя (фрагмент в 05) — дефект.
        let waiting = PresentSync::none().wait_for_camera_acquire(&no_wait);
        assert_eq!(
            waiting.camera_acquire_wait_stage(),
            vk::PipelineStageFlags2::ALL_COMMANDS
        );
        // `NoWait` не добавляет семафор; экспорт не затронут.
        assert_eq!(waiting.camera_acquire_semaphore(), None);
        assert!(waiting.release_export().is_none());
    }
}
