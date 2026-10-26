//! Сборщик `VkSubmitInfo2` (synchronization2) с именованными намерениями вместо россыпи массивов.

use ash::vk;

use super::import::WaitSemaphore;
use super::release::{ExportableSemaphore, PendingReleaseSignal};
use super::timeline::{SignalReservation, TimelineSemaphore};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::live_objects::{OwnedFence, OwnedSemaphore};

/// Один сабмит: что ждать, что исполнить, что просигналить.
///
/// Объекты, на которые сабмит ссылается, должны жить, пока GPU не закончит его: за этим следит
/// вызывающий (fence/timeline/`wait_idle`), сборщик лишь копирует дескрипторы.
///
/// ```ignore
/// QueueSubmission::new()
///     .wait_for_imported_fence(&acquire, vk::PipelineStageFlags2::COMPUTE_SHADER)
///     .run_command_buffer(command_buffer)
///     .signal_for_export(&release_semaphore, vk::PipelineStageFlags2::COMPUTE_SHADER)
///     .signal_fence(&fence)
///     .submit(&context)?;
/// ```
#[derive(Default)]
pub struct QueueSubmission<'resources> {
    waits: Vec<vk::SemaphoreSubmitInfo<'static>>,
    command_buffers: Vec<vk::CommandBufferSubmitInfo<'static>>,
    signals: Vec<vk::SemaphoreSubmitInfo<'static>>,
    /// Timeline-сигналы отдельно: нужна проверка монотонности и запись значения после успеха.
    timeline_signals: Vec<(&'resources TimelineSemaphore, u64)>,
    /// Семафоры для экспорта; больше одного на сабмит — ошибка (`submit` её сообщит).
    export_signals: Vec<&'resources ExportableSemaphore>,
    completion_fence: Option<vk::Fence>,
}

/// Итог успешного сабмита: токены, которые существуют только после отправки.
#[derive(Debug)]
pub struct SubmittedWork<'resources> {
    release_signal: Option<PendingReleaseSignal<'resources>>,
}

impl<'resources> SubmittedWork<'resources> {
    /// Забирает токен экспорта release-fence; `None`, если сабмит не просил сигнала для экспорта
    /// (или токен уже забран).
    pub fn take_release_signal(&mut self) -> Option<PendingReleaseSignal<'resources>> {
        self.release_signal.take()
    }
}

impl<'resources> QueueSubmission<'resources> {
    /// Пустой сабмит (допустим: только сигналы).
    pub fn new() -> Self {
        Self::default()
    }

    /// Ждать импортированный fence на стадии `stage`. `WaitSemaphore::NoWait` — ничего не добавляет.
    #[must_use]
    pub fn wait_for_imported_fence(mut self, wait: &WaitSemaphore, stage: vk::PipelineStageFlags2) -> Self {
        if let Some(semaphore) = wait.semaphore() {
            self.waits.push(semaphore_info(semaphore, 0, stage));
        }
        self
    }

    /// Ждать, пока timeline достигнет `value`.
    #[must_use]
    pub fn wait_for_timeline(
        mut self,
        timeline: &TimelineSemaphore,
        value: u64,
        stage: vk::PipelineStageFlags2,
    ) -> Self {
        self.waits.push(semaphore_info(timeline.handle(), value, stage));
        self
    }

    /// Исполнить записанный командный буфер (порядок вызовов = порядок исполнения).
    #[must_use]
    pub fn run_command_buffer(mut self, command_buffer: vk::CommandBuffer) -> Self {
        self.command_buffers
            .push(vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer));
        self
    }

    /// Просигналить обычный бинарный семафор на стадии `stage`.
    #[must_use]
    pub fn signal_binary(mut self, semaphore: &OwnedSemaphore, stage: vk::PipelineStageFlags2) -> Self {
        self.signals.push(semaphore_info(semaphore.handle(), 0, stage));
        self
    }

    /// Просигналить timeline значением `value` (должно быть больше отправленных — проверка в `submit`).
    #[must_use]
    pub fn signal_timeline(
        mut self,
        timeline: &'resources TimelineSemaphore,
        value: u64,
        stage: vk::PipelineStageFlags2,
    ) -> Self {
        self.signals.push(semaphore_info(timeline.handle(), value, stage));
        self.timeline_signals.push((timeline, value));
        self
    }

    /// Просигналить экспортируемый семафор: после `submit` из результата можно взять
    /// [`PendingReleaseSignal`] и экспортировать release-fence. Не более одного на сабмит.
    #[must_use]
    pub fn signal_for_export(
        mut self,
        semaphore: &'resources ExportableSemaphore,
        stage: vk::PipelineStageFlags2,
    ) -> Self {
        self.signals.push(semaphore_info(semaphore.handle(), 0, stage));
        self.export_signals.push(semaphore);
        self
    }

    /// Сигнал `VkFence` по завершении всего сабмита (для ожидания на CPU).
    #[must_use]
    pub fn signal_fence(mut self, fence: &OwnedFence) -> Self {
        self.completion_fence = Some(fence.handle());
        self
    }

    /// Отправляет сабмит в очередь контекста.
    ///
    /// Ошибка до обращения к драйверу (`InvalidArgument`/`InvalidState`) и ошибка драйвера не
    /// оставляют следов: пометки «занято» и значения timeline откатываются/не записываются.
    pub fn submit(self, context: &GpuContext) -> Result<SubmittedWork<'resources>, GpuError> {
        if self.export_signals.len() > 1 {
            return Err(GpuError::InvalidArgument {
                what: "в одном сабмите можно просить экспорт только одного семафора".to_owned(),
            });
        }
        // Значения timeline занимаются атомарно (проверка и запись — один шаг); при любой ошибке
        // до отправки или в ней все резервы откатываются.
        let mut reservations: Vec<(&TimelineSemaphore, SignalReservation)> = Vec::new();
        for (timeline, value) in &self.timeline_signals {
            match timeline.reserve_signal_value(*value) {
                Ok(reservation) => reservations.push((timeline, reservation)),
                Err(error) => {
                    cancel_all(&reservations);
                    return Err(error);
                }
            }
        }
        let export_semaphore = self.export_signals.first().copied();
        if let Some(export_semaphore) = export_semaphore
            && let Err(error) = export_semaphore.reserve_for_signal()
        {
            cancel_all(&reservations);
            return Err(error);
        }

        let submit_info = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&self.waits)
            .command_buffer_infos(&self.command_buffers)
            .signal_semaphore_infos(&self.signals);
        let result = context.submit(&[submit_info], self.completion_fence.unwrap_or_default());
        if let Err(error) = result {
            cancel_all(&reservations);
            if let Some(export_semaphore) = export_semaphore {
                export_semaphore.cancel_reservation();
            }
            return Err(error);
        }
        Ok(SubmittedWork {
            release_signal: export_semaphore.map(ExportableSemaphore::pending_signal),
        })
    }
}

/// Откатывает все занятые значения timeline (в обратном порядке занятия).
fn cancel_all(reservations: &[(&TimelineSemaphore, SignalReservation)]) {
    for (timeline, reservation) in reservations.iter().rev() {
        timeline.cancel_reservation(*reservation);
    }
}

fn semaphore_info(
    semaphore: vk::Semaphore,
    value: u64,
    stage: vk::PipelineStageFlags2,
) -> vk::SemaphoreSubmitInfo<'static> {
    vk::SemaphoreSubmitInfo::default()
        .semaphore(semaphore)
        .value(value)
        .stage_mask(stage)
}
