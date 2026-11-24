//! Вход и выход кадра сессии показа: откуда взята картинка swapchain и что с ней стало (решение 214, сессия 16a).

use gpu_vulkan::swapchain::{DisplayFrame, PresentOutcome as GpuPresentOutcome};

/// Картинка swapchain, выданная сессии на рисование.
pub enum AcquiredFrame {
    /// Только что выдана целью: итоги её слота кольца ещё не собраны.
    Fresh(DisplayFrame),
    /// Выдана на тике, который был пропущен как «ничего не изменилось», и придержана до следующего рисуемого
    /// тика: итоги слота уже собраны, повторять сбор нельзя.
    HeldFromSkippedTick(DisplayFrame),
}

impl AcquiredFrame {
    /// Сама картинка, когда происхождение не важно (рисовальщик без пропуска тиков).
    pub fn into_display_frame(self) -> DisplayFrame {
        match self {
            Self::Fresh(frame) | Self::HeldFromSkippedTick(frame) => frame,
        }
    }
}

impl FrameDisposition {
    /// Итог показа нарисованного тика: показ принят очередью или ошибка цели.
    pub(super) fn of_present(
        outcome: Result<GpuPresentOutcome, gpu_vulkan::swapchain::SwapchainError>,
    ) -> Result<Self, super::CameraDrawError> {
        outcome.map(Self::Presented).map_err(super::CameraDrawError::Swapchain)
    }
}

/// Что стало с выданной картинкой.
pub enum FrameDisposition {
    /// Кадр записан и отправлен на показ (итог показа внутри; ошибка показа — `CameraDrawError`).
    Presented(GpuPresentOutcome),
    /// Тик пропущен (решение 214): ничего не записано и не показано, картинка не рисовалась. Владелец обязан её
    /// придержать и вернуть на следующем рисуемом тике как [`AcquiredFrame::HeldFromSkippedTick`] либо отдать цели
    /// на отказ (`discard_unpresented_frame`) перед пересозданием и отсоединением.
    SkippedUnchanged(DisplayFrame),
}
