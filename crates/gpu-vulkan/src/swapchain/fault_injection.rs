//! Швы отказов для тестов: заставляют цель показа пройти пути ошибок, которые на настоящем
//! драйвере детерминированно не вызвать (зависший GPU, сбой посреди пересоздания).
//!
//! Шов одноразовый и взводится только явно: в боевых сборках (без feature `fault-injection`)
//! взвести его нечем, поэтому [`FaultPlan::trigger`] всегда возвращает `false` и ничего не меняет.

/// Какой отказ изобразить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedFault {
    /// Следующее ожидание GPU (пересоздание / отсоединение) сообщает таймаут, не дожидаясь:
    /// так изображается зависший GPU. Vulkan-объекты при этом НЕ уничтожаются (утечка).
    StuckGpu,
    /// Следующее пересоздание падает ПОСЛЕ успешного создания нового swapchain (старый уже retired).
    RecreateFailsAfterSwapchainCreated,
    /// Следующий acquire падает ПОСЛЕ успешного `vkAcquireNextImageKHR` (семафор слота уже сигнален).
    AcquireFailsAfterImageAcquired,
    /// Цель отказывается от present-fence'ов: ожидание показа идёт запасным путём (`vkDeviceWaitIdle`),
    /// как на устройстве без `VK_EXT_swapchain_maintenance1`.
    #[cfg_attr(
        not(any(test, feature = "fault-injection")),
        expect(dead_code, reason = "строится только тестовым швом")
    )]
    WithoutPresentFences,
}

/// Взведённые швы цели.
#[derive(Debug, Default)]
pub(super) struct FaultPlan {
    armed: Vec<InjectedFault>,
}

impl FaultPlan {
    /// Взводит шов (повторный вызов добавляет ещё одно срабатывание).
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn arm(&mut self, fault: InjectedFault) {
        self.armed.push(fault);
    }

    /// Срабатывает ли шов сейчас; сработавший шов снимается.
    pub(super) fn trigger(&mut self, fault: InjectedFault) -> bool {
        match self.armed.iter().position(|armed| *armed == fault) {
            Some(position) => {
                self.armed.swap_remove(position);
                true
            }
            None => false,
        }
    }
}
