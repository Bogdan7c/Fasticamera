//! Отслеживание окончания показа: fence на каждый `vkQueuePresentKHR` (`VK_EXT_swapchain_maintenance1`).
//!
//! ЗАЧЕМ. Уничтожить swapchain и семафоры «рендер готов» можно, только когда движок показа
//! закончил с ними. Fence'ов кольца кадров для этого мало: они сигналят окончание сабмита, а не
//! показа. Без расширения дождаться показа можно только `vkDeviceWaitIdle` — он БЕЗ таймаута и
//! блокирует поток рендера целиком (а у оболочки на освобождение окна бюджет в сотни миллисекунд).
//! С расширением `vkQueuePresentKHR` принимает fence (`VkSwapchainPresentFenceInfoEXT`), который
//! сигналится по окончании показа, и ждать его можно с таймаутом.
//!
//! ИНВАРИАНТЫ.
//! * Fence в present передаётся только несигнальный и не занятый другим показом: пул выдаёт
//!   лишь `Idle`-записи (сброшенные), запись `Pending` возвращается в пул после сигнала.
//! * Показ, чей исход неизвестен (`vkQueuePresentKHR` вернул ошибку, пул исчерпан), честно
//!   помечается «пробелом покрытия»: тогда [`PresentTracking::wait_presents_done`] сообщает, что
//!   нужен запасной путь, а не делает вид, что всё дождались.
//! * Все ожидания ограничены таймаутом.

use std::time::{Duration, Instant};

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::frame_ring::timeout_nanos;
use crate::live_objects::OwnedFence;

/// Чем закончилось ожидание показов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PresentsDone {
    /// Все отслеживаемые показы закончены, пробелов покрытия нет: swapchain уничтожать можно.
    Confirmed,
    /// Дождаться показов fence'ами нельзя (нет расширения или есть показ с неизвестным исходом):
    /// вызывающий обязан использовать запасной путь — ожидание простоя устройства.
    DeviceWideWaitRequired,
}

/// Состояние записи пула.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryState {
    /// Несигнальный, свободен.
    Idle,
    /// Передан в present, сигнала ещё не видели.
    Pending,
    /// Present вернул ошибку: неизвестно, придёт ли сигнал. Из оборота выведен до простоя устройства.
    Lost,
}

struct FenceEntry {
    fence: OwnedFence,
    state: EntryState,
}

/// Пул fence'ов показа.
pub(super) struct PresentFencePool {
    context: GpuContext,
    entries: Vec<FenceEntry>,
    /// Верхняя граница размера пула; исчерпание = пробел покрытия, а не ошибка.
    capacity: usize,
    coverage_gap: bool,
}

/// Способ узнать, что показы закончены.
pub(super) enum PresentTracking {
    /// Есть `VK_EXT_swapchain_maintenance1`: fence на каждый показ.
    PerPresentFence(PresentFencePool),
    /// Расширения нет (ПК-стенд без него): остаётся только `vkDeviceWaitIdle` — ограниченного
    /// по времени способа нет. Это известный пробел, он задокументирован и виден в `info()`.
    DeviceWideWait,
}

impl PresentTracking {
    /// Выбирает способ по возможностям контекста.
    pub(super) fn for_context(context: &GpuContext, pool_capacity: usize) -> Self {
        if context.present_fences_supported() {
            Self::PerPresentFence(PresentFencePool::new(context, pool_capacity))
        } else {
            Self::DeviceWideWait
        }
    }

    /// Отслеживаются ли показы fence'ами (для `info()` и тестов).
    pub(super) fn is_fence_based(&self) -> bool {
        matches!(self, Self::PerPresentFence(_))
    }

    /// Готовит fence под очередной показ. `None` — отслеживания нет (или пул исчерпан и это учтено
    /// как пробел покрытия): показ идёт без fence'а.
    pub(super) fn begin_present(&mut self) -> Result<Option<vk::Fence>, GpuError> {
        match self {
            Self::PerPresentFence(pool) => pool.begin_present(),
            Self::DeviceWideWait => Ok(None),
        }
    }

    /// `vkQueuePresentKHR` вернул ошибку: исход показа с этим fence'ом неизвестен.
    pub(super) fn present_failed(&mut self, fence: Option<vk::Fence>) {
        if let (Self::PerPresentFence(pool), Some(fence)) = (self, fence) {
            pool.mark_lost(fence);
        }
    }

    /// Ждёт окончания всех отслеживаемых показов не дольше `timeout`.
    pub(super) fn wait_presents_done(&mut self, timeout: Duration) -> Result<PresentsDone, GpuError> {
        match self {
            Self::PerPresentFence(pool) => pool.wait_all_pending(timeout),
            Self::DeviceWideWait => Ok(PresentsDone::DeviceWideWaitRequired),
        }
    }

    /// Устройство простаивает целиком: всё, что было неизвестным, закончилось.
    pub(super) fn settle_after_device_idle(&mut self) -> Result<(), GpuError> {
        match self {
            Self::PerPresentFence(pool) => pool.settle_after_device_idle(),
            Self::DeviceWideWait => Ok(()),
        }
    }

    /// Сколько показов сейчас ждут сигнала (для тестов учёта).
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn pending_count(&self) -> usize {
        match self {
            Self::PerPresentFence(pool) => pool.count_in(EntryState::Pending),
            Self::DeviceWideWait => 0,
        }
    }
}

impl PresentFencePool {
    fn new(context: &GpuContext, capacity: usize) -> Self {
        Self {
            context: context.clone(),
            entries: Vec::new(),
            capacity,
            coverage_gap: false,
        }
    }

    #[cfg(any(test, feature = "fault-injection"))]
    fn count_in(&self, state: EntryState) -> usize {
        self.entries.iter().filter(|entry| entry.state == state).count()
    }

    /// Возвращает в оборот записи, чей сигнал уже пришёл (без ожидания).
    fn reclaim_signaled(&mut self) -> Result<(), GpuError> {
        let device = self.context.device();
        for entry in &mut self.entries {
            if entry.state != EntryState::Pending {
                continue;
            }
            // SAFETY: fence принадлежит этому устройству.
            match unsafe { device.get_fence_status(entry.fence.handle()) } {
                Ok(true) => {
                    // SAFETY: fence сигнален и ни в одной очереди не ждёт.
                    run_step(
                        unsafe { device.reset_fences(&[entry.fence.handle()]) },
                        "vkResetFences(present)",
                    )?;
                    entry.state = EntryState::Idle;
                }
                Ok(false) => {}
                Err(result) => {
                    return Err(GpuError::Vulkan {
                        step: "vkGetFenceStatus(present)",
                        result,
                    });
                }
            }
        }
        Ok(())
    }

    fn begin_present(&mut self) -> Result<Option<vk::Fence>, GpuError> {
        self.reclaim_signaled()?;
        let free = self.entries.iter().position(|entry| entry.state == EntryState::Idle);
        let index = match free {
            Some(index) => index,
            None if self.entries.len() < self.capacity => {
                let info = vk::FenceCreateInfo::default();
                // SAFETY: устройство валидно; info — живая локальная структура.
                let handle = run_step(
                    unsafe { self.context.device().create_fence(&info, None) },
                    "vkCreateFence(present)",
                )?;
                self.entries.push(FenceEntry {
                    fence: OwnedFence::from_raw(&self.context, handle),
                    state: EntryState::Idle,
                });
                self.entries.len() - 1
            }
            None => {
                // Пул исчерпан: показ пойдёт без fence'а. Честно фиксируем пробел, не прячем.
                self.coverage_gap = true;
                tracing::warn!(
                    target: "fasticamera::gpu::swapchain",
                    capacity = self.capacity,
                    "пул present-fence'ов исчерпан: показ без fence'а, при освобождении понадобится ожидание простоя устройства"
                );
                return Ok(None);
            }
        };
        let entry = &mut self.entries[index];
        entry.state = EntryState::Pending;
        Ok(Some(entry.fence.handle()))
    }

    fn mark_lost(&mut self, fence: vk::Fence) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.fence.handle() == fence) {
            entry.state = EntryState::Lost;
        }
        self.coverage_gap = true;
    }

    fn wait_all_pending(&mut self, timeout: Duration) -> Result<PresentsDone, GpuError> {
        let pending: Vec<vk::Fence> = self
            .entries
            .iter()
            .filter(|entry| entry.state == EntryState::Pending)
            .map(|entry| entry.fence.handle())
            .collect();
        if !pending.is_empty() {
            let started = Instant::now();
            // SAFETY: fence'ы принадлежат этому устройству и переданы в present.
            let wait = unsafe {
                self.context
                    .device()
                    .wait_for_fences(&pending, true, timeout_nanos(timeout))
            };
            match wait {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => {
                    tracing::warn!(
                        target: "fasticamera::gpu::swapchain",
                        waited_ms = started.elapsed().as_millis(),
                        "показ не закончился за отведённое время"
                    );
                    return Err(GpuError::Timeout {
                        step: "vkWaitForFences(present)",
                    });
                }
                Err(result) => {
                    return Err(GpuError::Vulkan {
                        step: "vkWaitForFences(present)",
                        result,
                    });
                }
            }
            self.reclaim_signaled()?;
        }
        Ok(if self.coverage_gap {
            PresentsDone::DeviceWideWaitRequired
        } else {
            PresentsDone::Confirmed
        })
    }

    fn settle_after_device_idle(&mut self) -> Result<(), GpuError> {
        let device = self.context.device();
        for entry in &mut self.entries {
            // SAFETY: устройство простаивает: ни один fence ни в чём не участвует. Сброс
            // сигнального и несигнального fence'а одинаково допустим.
            run_step(
                unsafe { device.reset_fences(&[entry.fence.handle()]) },
                "vkResetFences(present)",
            )?;
            entry.state = EntryState::Idle;
        }
        self.coverage_gap = false;
        Ok(())
    }
}
