//! Запуск потока рендера и его подключение к маршрутизатору (решение владельца 31).
//!
//! Модуль нейтрален к платформе: бэкенд и часы кадров приходят снаружи (на телефоне — Vulkan и
//! vsync, в тестах — fake), поэтому весь путь «событие оболочки → ящик → поток → квитанция»
//! проверяется на ПК настоящим потоком рендера.
//!
//! Результат — [`RenderRuntime`]: маршрут событий для маршрутизатора и источник наблюдений для
//! сценариев. Если рендер не поднялся, [`RenderRuntime::unavailable`] даёт запасной маршрут,
//! который подтверждает освобождение surface сразу, и источник с причиной отказа.

use std::sync::Arc;

use render_loop::backend::DisplayBackend;
use render_loop::thread::{FrameClock, RenderLoop, RenderLoopHandle, RenderLoopParts, SpawnError};

use crate::gpu_services::{RenderObservation, RenderStatusSource, UnavailableRender, ValidationState};
use crate::render_route::{HoldAndConfirmRoute, InboxRoute, RenderRoute};
use crate::viewfinder_control::{InboxRenderControl, RenderControl, UnavailableRenderControl};

/// Что получает маршрутизатор после запуска рендера (или после отказа).
pub struct RenderRuntime {
    /// Куда маршрутизатор передаёт surface и события жизненного цикла.
    pub route: Box<dyn RenderRoute>,
    /// Откуда сценарии берут состояние рендера.
    pub status: Arc<dyn RenderStatusSource>,
    /// Куда сценарии шлют регулировки рендера (видоискатель, линия кадра).
    pub control: Arc<dyn RenderControl>,
}

impl RenderRuntime {
    /// Запускает поток рендера. Ошибка — только неверный конфиг или не создавшийся поток;
    /// инициализация GPU происходит раньше, при создании бэкенда.
    pub fn spawn<B, C>(parts: RenderLoopParts<B, C>, validation: ValidationState) -> Result<Self, SpawnError>
    where
        B: DisplayBackend,
        C: FrameClock,
    {
        let handle = RenderLoop::spawn(parts)?;
        let route = InboxRoute::new(handle.inbox());
        let control = InboxRenderControl::new(handle.inbox());
        Ok(Self {
            route: Box::new(route),
            control: Arc::new(control),
            status: Arc::new(LiveRenderStatus { handle, validation }),
        })
    }

    /// Рендера нет: surface держит и подтверждает запасной маршрут, причина видна сценариям.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        tracing::error!(target: "fasticamera::render", reason = %reason, "рендер недоступен, surface подтверждается сразу");
        Self {
            route: Box::new(HoldAndConfirmRoute::new()),
            status: Arc::new(UnavailableRender::new(reason.clone())),
            control: Arc::new(UnavailableRenderControl::new(reason)),
        }
    }
}

/// Наблюдения за живым потоком рендера. Владеет дескриптором: поток живёт, пока жив источник.
struct LiveRenderStatus {
    handle: RenderLoopHandle,
    validation: ValidationState,
}

impl RenderStatusSource for LiveRenderStatus {
    fn observe(&self) -> RenderObservation {
        RenderObservation::Live {
            status: Box::new(self.handle.status()),
            validation: self.validation,
        }
    }
}

#[cfg(test)]
#[path = "render_wiring_tests.rs"]
mod tests;
