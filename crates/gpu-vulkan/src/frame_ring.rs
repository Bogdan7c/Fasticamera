//! `FrameRing` — кольцо кадров в полёте: ресурсы синхронизации CPU↔GPU для показа.
//!
//! Что внутри:
//! * N слотов (N = `frames_in_flight`): у каждого свой командный буфер, семафор «картинка получена»
//!   (его сигналит `vkAcquireNextImageKHR`, ждёт сабмит) и fence «слот свободен» (сигналит сабмит);
//! * семафоры «рендер готов» — по одному НА КАРТИНКУ swapchain (их ждёт present). Они привязаны к
//!   картинке, а не к слоту: один слот может получить любую картинку, а present картинки ещё
//!   использует семафор, когда слот уже свободен (VUID `vkQueueSubmit2-semaphore-03868` и классическая
//!   гонка «render finished»). Число картинок меняется с swapchain — семафоры пересоздаются.
//!
//! Кольцо НЕ знает про swapchain и картинки: оно выдаёт слоты и отправляет работу. Что рисовать и как
//! показывать — решает владелец (`swapchain::DisplayTarget`). Это держит границу узкой: кольцо можно
//! тестировать без surface.
//!
//! Инварианты:
//! * слоты выдаются по кругу; слот выдан — `next_slot` не двигается, пока работа не отправлена;
//! * fence слота сбрасывается ТОЛЬКО прямо перед сабмитом, поэтому пропущенный кадр (таймаут
//!   acquire, устаревший swapchain) не оставляет несигнального fence;
//! * все ожидания ограничены таймаутом — бесконечных ожиданий нет.

use std::time::Duration;

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedCommandPool, OwnedFence, OwnedSemaphore};
use crate::swapchain::FramesInFlight;

/// Максимальное ожидание, передаваемое Vulkan, в наносекундах. Это `i64::MAX`, а не «вечность»
/// (`u64::MAX` у Vulkan означает бесконечное ожидание, чего в crate быть не должно): 292 года.
const MAX_WAIT_NANOS: u64 = i64::MAX as u64;

/// Переводит длительность в наносекунды для Vulkan, с насыщением на [`MAX_WAIT_NANOS`].
pub(crate) fn timeout_nanos(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_nanos()).map_or(MAX_WAIT_NANOS, |nanos| nanos.min(MAX_WAIT_NANOS))
}

/// Номер слота кольца (непрозрачный: снаружи кольца его не придумать).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SlotIndex(usize);

/// Итог ожидания очередного слота.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotWait {
    /// Слот свободен: GPU закончил его прошлый кадр.
    Ready(SlotIndex),
    /// GPU не освободил слот за отведённое время (кадры не успевают).
    TimedOut,
}

/// Ресурсы одного слота.
struct FrameSlot {
    command_buffer: vk::CommandBuffer,
    image_acquired: OwnedSemaphore,
    in_flight: OwnedFence,
}

/// Кольцо кадров в полёте.
pub struct FrameRing {
    // Порядок полей: пул (с буферами) и слоты освобождаются раньше контекста внутри Owned*.
    slots: Vec<FrameSlot>,
    render_finished: Vec<OwnedSemaphore>,
    /// Пул владеет командными буферами слотов; читать его после создания не нужно, только держать живым.
    _command_pool: OwnedCommandPool,
    next_slot: usize,
    context: GpuContext,
}

impl FrameRing {
    /// Создаёт кольцо: `frames_in_flight` слотов и `image_count` семафоров «рендер готов».
    pub fn new(context: &GpuContext, frames_in_flight: FramesInFlight, image_count: usize) -> Result<Self, GpuError> {
        let device = context.device();
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(context.queue_family_index())
            // Каждый кадр переписывает свой буфер: нужен сброс отдельных буферов.
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: устройство валидно; pool_info — живая локальная структура.
        let command_pool = OwnedCommandPool::from_raw(
            context,
            run_step(
                unsafe { device.create_command_pool(&pool_info, None) },
                "vkCreateCommandPool",
            )?,
        );
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool.handle())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(
                u32::try_from(frames_in_flight.get()).map_err(|_| GpuError::InvalidArgument {
                    what: "frames_in_flight не помещается в u32".to_owned(),
                })?,
            );
        // SAFETY: пул только что создан этим устройством; буферы освободятся вместе с пулом.
        let command_buffers = run_step(
            unsafe { device.allocate_command_buffers(&allocate_info) },
            "vkAllocateCommandBuffers",
        )?;

        let mut slots = Vec::with_capacity(command_buffers.len());
        for command_buffer in command_buffers {
            slots.push(FrameSlot {
                command_buffer,
                image_acquired: create_semaphore(context)?,
                // Сигнальный: первый кадр слота не должен ждать вечно того, чего не было.
                in_flight: create_fence(context, vk::FenceCreateFlags::SIGNALED)?,
            });
        }
        let mut ring = Self {
            slots,
            render_finished: Vec::new(),
            _command_pool: command_pool,
            next_slot: 0,
            context: context.clone(),
        };
        ring.recreate_render_finished(image_count)?;
        Ok(ring)
    }

    /// Число слотов кольца.
    pub fn frames_in_flight(&self) -> usize {
        self.slots.len()
    }

    /// Пересоздаёт семафоры «рендер готов» под новое число картинок swapchain.
    ///
    /// Предусловие вызывающего: GPU и показ закончили с прежними семафорами (после `wait_for_idle`).
    /// При ошибке старый набор уже снят — вызывающий обязан пересоздать swapchain-ресурсы целиком.
    pub(crate) fn recreate_render_finished(&mut self, image_count: usize) -> Result<(), GpuError> {
        self.render_finished.clear();
        for _ in 0..image_count {
            self.render_finished.push(create_semaphore(&self.context)?);
        }
        Ok(())
    }

    /// Заменяет семафоры «картинка получена» всех слотов новыми несигнальными.
    ///
    /// Нужно после сорванного кадра: `vkAcquireNextImageKHR` уже сигналит семафор слота, но
    /// сабмит, который его ждёт, не состоялся. Сигнальный семафор нельзя отдавать в следующий
    /// acquire (VUID-vkAcquireNextImageKHR-semaphore-01286), а «снять» сигнал можно только
    /// ожиданием в сабмите — которого нет. Поэтому семафор выбрасывается вместе с swapchain, из
    /// которого пришёл сигнал.
    ///
    /// Предусловие вызывающего: старый swapchain уже уничтожен, GPU и показ закончили (после
    /// `wait_for_idle`), кадр не выдан. При ошибке часть слотов может остаться со старыми
    /// семафорами — вызывающий повторит замену (операция идемпотентна).
    pub(crate) fn replace_image_acquired_semaphores(&mut self) -> Result<(), GpuError> {
        for slot in &mut self.slots {
            slot.image_acquired = create_semaphore(&self.context)?;
        }
        Ok(())
    }

    /// Число семафоров «рендер готов» (равно числу картинок swapchain).
    pub fn render_finished_count(&self) -> usize {
        self.render_finished.len()
    }

    /// Ждёт, пока очередной слот освободится, но не дольше `timeout`. Слот НЕ занимается
    /// (`next_slot` не двигается) — это делает [`Self::end_and_submit`].
    pub(crate) fn wait_for_next_slot(&self, timeout: Duration) -> Result<SlotWait, GpuError> {
        let slot = &self.slots[self.next_slot];
        // SAFETY: fence принадлежит этому устройству.
        let wait = unsafe {
            self.context
                .device()
                .wait_for_fences(&[slot.in_flight.handle()], true, timeout_nanos(timeout))
        };
        match wait {
            Ok(()) => Ok(SlotWait::Ready(SlotIndex(self.next_slot))),
            Err(vk::Result::TIMEOUT) => Ok(SlotWait::TimedOut),
            Err(result) => Err(GpuError::Vulkan {
                step: "vkWaitForFences(слот кадра)",
                result,
            }),
        }
    }

    /// Семафор «картинка получена» слота (его сигналит `vkAcquireNextImageKHR`).
    pub(crate) fn image_acquired_semaphore(&self, slot: SlotIndex) -> vk::Semaphore {
        self.slots[slot.0].image_acquired.handle()
    }

    /// Семафор «рендер готов» для картинки swapchain (его ждёт present).
    pub(crate) fn render_finished_semaphore(&self, image_index: u32) -> Result<vk::Semaphore, GpuError> {
        self.render_finished
            .get(image_index as usize)
            .map(OwnedSemaphore::handle)
            .ok_or_else(|| GpuError::InvalidArgument {
                what: format!(
                    "индекс картинки {image_index} вне кольца ({} семафоров)",
                    self.render_finished.len()
                ),
            })
    }

    /// Сбрасывает командный буфер слота и начинает запись (`ONE_TIME_SUBMIT`).
    pub(crate) fn begin_recording(&self, slot: SlotIndex) -> Result<vk::CommandBuffer, GpuError> {
        let command_buffer = self.slots[slot.0].command_buffer;
        let device = self.context.device();
        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: fence слота сигнален (мы ждали его в wait_for_next_slot), значит GPU буфер не использует;
        // пул создан с RESET_COMMAND_BUFFER; запись идёт из одного потока.
        unsafe {
            run_step(
                device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty()),
                "vkResetCommandBuffer",
            )?;
            run_step(
                device.begin_command_buffer(command_buffer, &begin_info),
                "vkBeginCommandBuffer",
            )?;
        }
        Ok(command_buffer)
    }

    /// Заканчивает запись и отправляет слот: ждёт «картинка получена», сигналит «рендер готов»
    /// картинки `image_index` и fence слота. После успеха слот занят, `next_slot` двигается.
    ///
    /// Если сам сабмит вернул ошибку (потеря устройства, нехватка памяти), fence слота остаётся
    /// несигнальным: следующее ожидание слота упрётся в таймаут — это фатальное состояние,
    /// вызывающий должен пересоздать контекст.
    pub(crate) fn end_and_submit(&mut self, slot: SlotIndex, image_index: u32) -> Result<(), GpuError> {
        let render_finished = self.render_finished_semaphore(image_index)?;
        let device = self.context.device();
        let frame = &self.slots[slot.0];
        // SAFETY: буфер в состоянии записи (begin_recording), запись идёт из этого потока.
        run_step(
            unsafe { device.end_command_buffer(frame.command_buffer) },
            "vkEndCommandBuffer",
        )?;
        // SAFETY: fence принадлежит устройству и сигнален (слот свободен); сброс прямо перед сабмитом.
        run_step(
            unsafe { device.reset_fences(&[frame.in_flight.handle()]) },
            "vkResetFences",
        )?;

        let waits = [vk::SemaphoreSubmitInfo::default()
            .semaphore(frame.image_acquired.handle())
            // Картинка нужна только с момента записи в цветовое вложение.
            .stage_mask(vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT)];
        let signals = [vk::SemaphoreSubmitInfo::default()
            .semaphore(render_finished)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let command_buffers = [vk::CommandBufferSubmitInfo::default().command_buffer(frame.command_buffer)];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&waits)
            .command_buffer_infos(&command_buffers)
            .signal_semaphore_infos(&signals);
        self.context.submit(&[submit], frame.in_flight.handle())?;

        self.next_slot = (self.next_slot + 1) % self.slots.len();
        Ok(())
    }

    /// Ждёт, пока GPU закончит работу ВСЕХ слотов, но не дольше `timeout` (общий бюджет).
    pub(crate) fn wait_for_idle(&self, timeout: Duration) -> Result<(), GpuError> {
        let fences: Vec<vk::Fence> = self.slots.iter().map(|slot| slot.in_flight.handle()).collect();
        // SAFETY: все fence'ы принадлежат этому устройству.
        let wait = unsafe {
            self.context
                .device()
                .wait_for_fences(&fences, true, timeout_nanos(timeout))
        };
        match wait {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => Err(GpuError::Timeout {
                step: "vkWaitForFences(кольцо кадров)",
            }),
            Err(result) => Err(GpuError::Vulkan {
                step: "vkWaitForFences(кольцо кадров)",
                result,
            }),
        }
    }

    /// Пул команд (нужен только для учёта в тестах владения).
    #[cfg(test)]
    fn pool_handle(&self) -> vk::CommandPool {
        self._command_pool.handle()
    }
}

fn create_semaphore(context: &GpuContext) -> Result<OwnedSemaphore, GpuError> {
    let info = vk::SemaphoreCreateInfo::default();
    // SAFETY: устройство валидно; info — живая локальная структура.
    let handle = run_step(
        unsafe { context.device().create_semaphore(&info, None) },
        "vkCreateSemaphore",
    )?;
    Ok(OwnedSemaphore::from_raw(context, handle))
}

fn create_fence(context: &GpuContext, flags: vk::FenceCreateFlags) -> Result<OwnedFence, GpuError> {
    let info = vk::FenceCreateInfo::default().flags(flags);
    // SAFETY: устройство валидно; info — живая локальная структура.
    let handle = run_step(unsafe { context.device().create_fence(&info, None) }, "vkCreateFence")?;
    Ok(OwnedFence::from_raw(context, handle))
}

#[cfg(test)]
mod tests;
