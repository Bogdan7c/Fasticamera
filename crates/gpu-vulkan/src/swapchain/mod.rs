//! Показ на экран: `DisplayTarget` — владелец surface, swapchain и кольца кадров в полёте.
//!
//! Архитектура (пакет A3, сессия 03):
//! * [`policy`] — ЧИСТАЯ логика выбора формата, числа картинок, размера, `preTransform`,
//!   composite alpha. Без GPU, тесты на ПК с синтетическими возможностями surface;
//! * [`surface`] — [`PresentationSurface`] (что именно показываем: окно Android или headless-стенд ПК),
//!   создание `VkSurfaceKHR` и запрос возможностей;
//! * [`config`] — настройки цели: число кадров в полёте, таймауты, предпочтение глубины цвета;
//! * [`frame`] — типы кадра: [`DisplayFrame`], исходы acquire и present;
//! * [`display_timing`] — `VK_GOOGLE_display_timing`: время показа и прошлые показы;
//! * `state`/`resources` (внутри) — состояние живой цели и ресурсы swapchain;
//! * [`target`] — публичный фасад [`DisplayTarget`] с жизненным циклом attach / detach.
//!
//! Инварианты:
//! * цель ВЛАДЕЕТ surface от `attach` до `detach`; `VkSurfaceKHR` уничтожается после swapchain;
//! * swapchain не уничтожается, пока GPU/показ не закончили с его картинками;
//! * бесконечных ожиданий нет: любое ожидание ограничено таймаутом из конфигурации;
//! * `ERROR_SURFACE_LOST` — типизированная ошибка [`SwapchainError::SurfaceLost`], не общий код Vulkan.

pub mod config;
pub mod display_timing;
pub mod frame;
pub mod policy;
pub mod surface;
pub mod target;

mod fault_injection;
mod present_fences;
pub mod present_sync;
mod resources;
mod state;

pub use self::config::{
    ColorDepthPreference, DisplayColorPreference, DisplayColorSpace, DisplayTargetConfig, FramesInFlight,
};
pub use self::display_timing::PastPresentation;
#[cfg(any(test, feature = "fault-injection"))]
pub use self::fault_injection::InjectedFault;
pub use self::frame::{AcquireOutcome, DisplayFrame, PresentOutcome, PresentRequest};
pub use self::policy::{PlanOutcome, SurfaceSupport, SwapchainPlan};
pub use self::present_sync::{PresentSync, SubmitProgress, SyncedPresent};
pub use self::surface::PresentationSurface;
pub use self::target::{AttachError, DetachError, DisplayTarget, DisplayTargetInfo, RecreateOutcome};

use ash::vk;

use crate::error::GpuError;

/// Ошибка работы цели показа.
///
/// Отдельный от [`GpuError`] тип: потеря surface и нулевой размер требуют от вызывающего
/// своей реакции (отпустить surface / подождать размер), их нельзя сливать с общим кодом Vulkan.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SwapchainError {
    /// `ERROR_SURFACE_LOST_KHR`: окно ушло (Android уничтожил surface). Цель надо отсоединить.
    #[error("surface потеряна (VK_ERROR_SURFACE_LOST_KHR) на шаге {step}")]
    SurfaceLost {
        /// Какой вызов вернул ошибку.
        step: &'static str,
    },
    /// Размер surface нулевой (окно свёрнуто): swapchain создать нельзя, надо дождаться размера.
    #[error("размер surface нулевой: swapchain создать нельзя")]
    ZeroExtent,
    /// Прочие ошибки GPU (код Vulkan, таймаут, неподдерживаемое, неверный аргумент или состояние).
    #[error(transparent)]
    Gpu(#[from] GpuError),
}

impl SwapchainError {
    /// Превращает код Vulkan в ошибку: `SURFACE_LOST` — типизированно, остальное — `GpuError::Vulkan`.
    pub(crate) fn from_vulkan(result: vk::Result, step: &'static str) -> Self {
        if result == vk::Result::ERROR_SURFACE_LOST_KHR {
            Self::SurfaceLost { step }
        } else {
            Self::Gpu(GpuError::Vulkan { step, result })
        }
    }
}

/// Превращает результат ash в `Result<_, SwapchainError>` с именем шага.
pub(crate) fn swapchain_step<T>(result: Result<T, vk::Result>, step: &'static str) -> Result<T, SwapchainError> {
    result.map_err(|code| SwapchainError::from_vulkan(code, step))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_lost_is_typed_and_other_codes_stay_vulkan_errors() {
        assert_eq!(
            SwapchainError::from_vulkan(vk::Result::ERROR_SURFACE_LOST_KHR, "vkQueuePresentKHR"),
            SwapchainError::SurfaceLost {
                step: "vkQueuePresentKHR"
            }
        );
        assert_eq!(
            SwapchainError::from_vulkan(vk::Result::ERROR_DEVICE_LOST, "vkQueuePresentKHR"),
            SwapchainError::Gpu(GpuError::Vulkan {
                step: "vkQueuePresentKHR",
                result: vk::Result::ERROR_DEVICE_LOST
            })
        );
    }
}
