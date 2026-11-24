//! Слоты стенда: командные буферы и fence-ы на `frames_in_flight` кадров (отдельно от `FrameRing` — показа нет).
//!
//! Слот либо свободен (GPU им не пользуется), либо занят отправленным кадром. Порядок занятости — очередь
//! «самый старый первым»: GPU исполняет сабмиты одной очереди по порядку, поэтому первым освобождается старший.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ash::vk;

use super::workload::SlotId;
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::frame_ring::timeout_nanos;
use crate::live_objects::{OwnedCommandPool, OwnedFence};
use crate::swapchain::FramesInFlight;

/// Шаг, которым ожидание fence проверяет отмену: отмена не ждёт полного таймаута, а лишний вызов на
/// насыщенном GPU ничего не стоит (fence сигналится раньше шага).
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Итог ожидания самого старого кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OldestWait {
    /// Fence сигнален.
    Signaled,
    /// Ожидание прервано отменой; кадр всё ещё в полёте.
    Interrupted,
}

/// Кольцо слотов стенда.
pub(crate) struct SlotRing {
    // Порядок полей: fence-ы и пул освобождаются раньше контекста внутри Owned*.
    fences: Vec<OwnedFence>,
    command_buffers: Vec<vk::CommandBuffer>,
    command_pool: Option<OwnedCommandPool>,
    /// Занятые слоты от самого старого к самому новому. Ёмкость задана заранее: в цикле кадров без аллокаций.
    in_flight: VecDeque<usize>,
    busy: Vec<bool>,
    /// С какого слота искать свободный: следующий после последнего отправленного (по кругу). Так стенд обходит
    /// ВСЕ слоты по очереди, даже когда GPU успевает освободить слот до следующего кадра: работа, у которой
    /// слотов меньше, чем у стенда, получает отказ на первом же круге, а не «когда повезёт» (тест 16a падал
    /// через раз), и кадры в полёте всегда пишут в разные ресурсы слотов, как в продуктовом кольце кадров.
    next_preferred: usize,
    context: GpuContext,
}

impl SlotRing {
    pub(crate) fn new(context: &GpuContext, frames_in_flight: FramesInFlight) -> Result<Self, GpuError> {
        let slot_count = frames_in_flight.get();
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
                "vkCreateCommandPool(saturation)",
            )?,
        );
        let allocate_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(command_pool.handle())
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(u32::try_from(slot_count).map_err(|_| GpuError::InvalidArgument {
                what: "frames_in_flight не помещается в u32".to_owned(),
            })?);
        // SAFETY: пул только что создан этим устройством; буферы освободятся вместе с пулом.
        let command_buffers = run_step(
            unsafe { device.allocate_command_buffers(&allocate_info) },
            "vkAllocateCommandBuffers(saturation)",
        )?;
        let mut fences = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            // SAFETY: устройство валидно. Fence несигнальный: слот свободен, ждать нечего, сигналит только сабмит.
            let handle = run_step(
                unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) },
                "vkCreateFence(saturation)",
            )?;
            fences.push(OwnedFence::from_raw(context, handle));
        }
        Ok(Self {
            fences,
            command_buffers,
            command_pool: Some(command_pool),
            in_flight: VecDeque::with_capacity(slot_count),
            busy: vec![false; slot_count],
            next_preferred: 0,
            context: context.clone(),
        })
    }

    /// Есть ли слот, в который можно записывать новый кадр.
    pub(crate) fn has_free_slot(&self) -> bool {
        self.in_flight.len() < self.busy.len()
    }

    /// Сколько кадров сейчас в полёте.
    pub(crate) fn in_flight_count(&self) -> usize {
        self.in_flight.len()
    }

    /// Свободный слот для следующего кадра (`None`, если все заняты). Поиск идёт по кругу от слота, следующего
    /// за последним отправленным: слоты используются по очереди (см. `next_preferred`).
    pub(crate) fn free_slot(&self) -> Option<SlotId> {
        let slot_count = self.busy.len();
        (0..slot_count)
            .map(|step| (self.next_preferred + step) % slot_count)
            .find(|&slot| !self.busy[slot])
            .map(SlotId)
    }

    /// Сбрасывает командный буфер слота и открывает запись. Слот обязан быть свободен.
    pub(crate) fn begin_recording(&self, slot: SlotId) -> Result<vk::CommandBuffer, GpuError> {
        self.ensure_free(slot)?;
        let command_buffer = self.command_buffers[slot.0];
        let device = self.context.device();
        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: слот свободен (fence прошлого кадра сигнален, GPU буфер не использует); пул создан с
        // RESET_COMMAND_BUFFER; запись идёт из одного потока (&mut стенда).
        unsafe {
            run_step(
                device.reset_command_buffer(command_buffer, vk::CommandBufferResetFlags::empty()),
                "vkResetCommandBuffer(saturation)",
            )?;
            run_step(
                device.begin_command_buffer(command_buffer, &begin_info),
                "vkBeginCommandBuffer(saturation)",
            )?;
        }
        Ok(command_buffer)
    }

    /// Закрывает запись и отправляет кадр; слот становится самым новым занятым. При ошибке слот остаётся свободным.
    pub(crate) fn end_and_submit(&mut self, slot: SlotId) -> Result<(), GpuError> {
        self.ensure_free(slot)?;
        let command_buffer = self.command_buffers[slot.0];
        let fence = self.fences[slot.0].handle();
        let device = self.context.device();
        // SAFETY: буфер в состоянии записи (begin_recording); fence принадлежит устройству и несигнален либо
        // сигнален прошлым кадром слота (слот свободен) — сброс прямо перед сабмитом.
        unsafe {
            run_step(
                device.end_command_buffer(command_buffer),
                "vkEndCommandBuffer(saturation)",
            )?;
            run_step(device.reset_fences(&[fence]), "vkResetFences(saturation)")?;
        }
        let command_buffer_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer)];
        let submit = vk::SubmitInfo2::default().command_buffer_infos(&command_buffer_infos);
        self.context.submit(&[submit], fence)?;
        self.busy[slot.0] = true;
        self.in_flight.push_back(slot.0);
        self.next_preferred = (slot.0 + 1) % self.busy.len();
        Ok(())
    }

    /// Неблокирующая проверка: сигнален ли fence самого старого кадра. Нет кадров в полёте — `Ok(false)`.
    pub(crate) fn oldest_is_signaled(&self) -> Result<bool, GpuError> {
        let Some(&slot) = self.in_flight.front() else {
            return Ok(false);
        };
        // SAFETY: fence принадлежит этому устройству; опрос не требует внешней синхронизации.
        run_step(
            unsafe { self.context.device().get_fence_status(self.fences[slot].handle()) },
            "vkGetFenceStatus(saturation)",
        )
    }

    /// Ждёт fence самого старого кадра не дольше `timeout`, проверяя отмену каждые [`CANCEL_POLL_INTERVAL`].
    /// `is_cancelled` — вопрос «прервать?»; для ожидания без отмены передают `|| false`.
    pub(crate) fn wait_oldest(
        &self,
        timeout: Duration,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<OldestWait, GpuError> {
        let Some(&slot) = self.in_flight.front() else {
            return Err(GpuError::InvalidState {
                what: "ожидание самого старого кадра без кадров в полёте".to_owned(),
            });
        };
        let fence = self.fences[slot].handle();
        let deadline = Instant::now() + timeout;
        loop {
            let slice = CANCEL_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
            // SAFETY: fence принадлежит этому устройству.
            let wait = unsafe {
                self.context
                    .device()
                    .wait_for_fences(&[fence], true, timeout_nanos(slice))
            };
            match wait {
                Ok(()) => return Ok(OldestWait::Signaled),
                Err(vk::Result::TIMEOUT) => {
                    if is_cancelled() {
                        return Ok(OldestWait::Interrupted);
                    }
                    if Instant::now() >= deadline {
                        return Err(GpuError::Timeout {
                            step: "vkWaitForFences(saturation)",
                        });
                    }
                }
                Err(result) => {
                    return Err(GpuError::Vulkan {
                        step: "vkWaitForFences(saturation)",
                        result,
                    });
                }
            }
        }
    }

    /// Снимает самый старый кадр с занятости. Вызывать ТОЛЬКО после наблюдённого сигнала его fence.
    pub(crate) fn release_oldest(&mut self) -> Option<SlotId> {
        let slot = self.in_flight.pop_front()?;
        self.busy[slot] = false;
        Some(SlotId(slot))
    }

    fn ensure_free(&self, slot: SlotId) -> Result<(), GpuError> {
        match self.busy.get(slot.0) {
            Some(false) => Ok(()),
            Some(true) => Err(GpuError::InvalidState {
                what: format!("слот {} занят кадром в полёте", slot.0),
            }),
            None => Err(GpuError::InvalidArgument {
                what: format!("слот {} вне кольца из {}", slot.0, self.busy.len()),
            }),
        }
    }
}

impl Drop for SlotRing {
    fn drop(&mut self) {
        // Занятые слоты, чей fence не сигнален, GPU ещё использует: уничтожать их нельзя (соглашение crate:
        // лучше утечка, чем уничтожение занятого).
        let gpu_still_busy = self.in_flight.iter().any(|&slot| {
            // SAFETY: fence принадлежит этому устройству; опрос без ожидания.
            !matches!(
                unsafe { self.context.device().get_fence_status(self.fences[slot].handle()) },
                Ok(true)
            )
        });
        if gpu_still_busy {
            tracing::error!(
                target: "fasticamera::gpu",
                "стенд насыщения уничтожен с кадрами в полёте: пул и fence-ы намеренно НЕ уничтожаются (утечка)"
            );
            std::mem::forget(std::mem::take(&mut self.fences));
            std::mem::forget(self.command_pool.take());
        }
    }
}
