//! Разовая работа на GPU: записать командный буфер, отправить, дождаться fence с таймаутом.
//!
//! Используется для readback, барьеров в тестах и диагностик. Бесконечных ожиданий нет:
//! срок берётся из `GpuContextConfig::submit_wait_timeout`.

use ash::vk;

use super::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedCommandPool, OwnedFence};

impl GpuContext {
    /// Записывает команды замыканием, отправляет их и ждёт завершения.
    ///
    /// При `GpuError::Timeout` пул команд и fence НЕ уничтожаются (намеренная утечка): GPU мог
    /// ещё выполнять буфер, а уничтожение занятых объектов — неопределённое поведение. Утечка
    /// видна в `live_objects()`, код ошибки означает «контекст подозрителен».
    pub(crate) fn run_one_shot(
        &self,
        record: impl FnOnce(vk::CommandBuffer) -> Result<(), GpuError>,
    ) -> Result<(), GpuError> {
        let device = self.device();

        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.queue_family_index())
            .flags(vk::CommandPoolCreateFlags::TRANSIENT);
        // SAFETY: устройство валидно; pool_info — живая локальная структура.
        let pool = OwnedCommandPool::from_raw(
            self,
            run_step(
                unsafe { device.create_command_pool(&pool_info, None) },
                "vkCreateCommandPool",
            )?,
        );

        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool.handle())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: пул только что создан этим устройством; буфер освободится вместе с пулом.
        let buffers = run_step(
            unsafe { device.allocate_command_buffers(&allocate_info) },
            "vkAllocateCommandBuffers",
        )?;
        let command_buffer = buffers.first().copied().ok_or(GpuError::InvalidState {
            what: "vkAllocateCommandBuffers не вернул буфер".to_owned(),
        })?;

        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: буфер в исходном состоянии, принадлежит этому потоку.
        run_step(
            unsafe { device.begin_command_buffer(command_buffer, &begin_info) },
            "vkBeginCommandBuffer",
        )?;
        record(command_buffer)?;
        // SAFETY: запись завершена в этом же потоке.
        run_step(
            unsafe { device.end_command_buffer(command_buffer) },
            "vkEndCommandBuffer",
        )?;

        let fence_info = vk::FenceCreateInfo::default();
        // SAFETY: устройство валидно.
        let fence = OwnedFence::from_raw(
            self,
            run_step(unsafe { device.create_fence(&fence_info, None) }, "vkCreateFence")?,
        );

        let command_buffer_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer)];
        let submit = vk::SubmitInfo2::default().command_buffer_infos(&command_buffer_infos);
        // Если отправка не удалась, GPU буфер не получил и объекты можно безопасно уничтожить (drop).
        self.submit(&[submit], fence.handle())?;

        let timeout_ns = u64::try_from(self.submit_wait_timeout().as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: fence принадлежит этому устройству.
        let wait_result = unsafe { device.wait_for_fences(&[fence.handle()], true, timeout_ns) };
        match wait_result {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => {
                tracing::error!(target: "fasticamera::gpu", "GPU не завершил разовую работу за таймаут; пул и fence оставлены (утечка)");
                std::mem::forget(fence);
                std::mem::forget(pool);
                Err(GpuError::Timeout {
                    step: "vkWaitForFences(one_shot)",
                })
            }
            Err(result) => {
                // Состояние GPU неизвестно (например, DEVICE_LOST) — тоже не уничтожаем занятое.
                std::mem::forget(fence);
                std::mem::forget(pool);
                Err(GpuError::Vulkan {
                    step: "vkWaitForFences(one_shot)",
                    result,
                })
            }
        }
    }
}
