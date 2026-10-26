//! Перевод типов `gpu-vulkan` в нейтральные типы `render-loop` (граница двух crate-ов).
//! Чистые функции без GPU: проверяются обычными юнит-тестами.

use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{DisplayTargetInfo, SwapchainError};
use shell_contract::surface::SurfaceSize;

use crate::backend::{BackendError, DisplayInfo};

/// Что случилось с кадром или цепочкой — для выбора реакции потока.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FailureKind {
    /// Surface потеряна: бэкенд её больше использовать не может.
    SurfaceLost,
    /// Любая другая ошибка.
    Other,
}

/// Классифицирует ошибку swapchain и строит человекочитаемую `BackendError`.
pub(super) fn classify_swapchain_error(operation: &str, error: &SwapchainError) -> (FailureKind, BackendError) {
    let kind = match error {
        SwapchainError::SurfaceLost { .. } => FailureKind::SurfaceLost,
        SwapchainError::ZeroExtent | SwapchainError::Gpu(_) => FailureKind::Other,
    };
    (kind, backend_error(operation, error))
}

/// Ошибка GPU как `BackendError`.
pub(super) fn gpu_backend_error(operation: &str, error: &GpuError) -> BackendError {
    backend_error(operation, error)
}

fn backend_error(operation: &str, reason: &dyn std::fmt::Display) -> BackendError {
    BackendError {
        operation: operation.to_owned(),
        reason: reason.to_string(),
    }
}

/// Снимок цепочки показа в нейтральном виде. `refresh_cycle` приходит отдельным вызовом цели.
pub(super) fn display_info(info: &DisplayTargetInfo, refresh_cycle: Option<std::time::Duration>) -> DisplayInfo {
    DisplayInfo {
        format: format!("{:?}", info.format),
        color_space: format!("{:?}", info.color_space),
        extent: SurfaceSize {
            width_px: info.extent.width,
            height_px: info.extent.height,
        },
        transform: format!("{:?}", info.transform),
        image_count: info.image_count,
        recreation_count: info.recreation_count,
        refresh_cycle,
    }
}
