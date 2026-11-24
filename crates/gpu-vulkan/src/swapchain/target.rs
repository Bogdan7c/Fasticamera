//! `DisplayTarget` — публичный фасад показа: владеет surface, swapchain и кольцом кадров.
//!
//! Жизненный цикл:
//! 1. [`DisplayTarget::attach`] — принимает [`PresentationSurface`] во владение; при ошибке ВОЗВРАЩАЕТ её
//!    вместе с ошибкой ([`AttachError`]): на Android окно нужно вернуть оболочке, чтобы подтвердить освобождение;
//! 2. цикл: `acquire_frame` → рисование вызывающего → `present`; при `NeedsRecreate` — `recreate`;
//! 3. [`DisplayTarget::detach`] — ждёт GPU, уничтожает swapchain, затем `VkSurfaceKHR`, и возвращает поверхность.
//!    `Drop` без `detach` делает то же самое (тот же порядок), но пишет в лог предупреждение.
//!
//! Вся механика — в `state`; здесь только намерения и граница владения.

use std::time::Duration;

use ash::vk;
use shell_contract::surface::SurfaceSize;

use super::SwapchainError;
use super::config::DisplayTargetConfig;
use super::display_timing::PastPresentation;
use super::frame::{AcquireOutcome, DisplayFrame, PresentOutcome, PresentRequest};
use super::present_sync::{PresentSync, SyncedPresent};
use super::state::TargetState;
use super::surface::PresentationSurface;
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::test_pattern::SurfaceTransform;

/// Почему пересоздание отложено.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroExtentReason {
    /// Размер surface нулевой (окно свёрнуто).
    ZeroExtent,
}

/// Итог `recreate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecreateOutcome {
    /// Swapchain пересоздан.
    Recreated {
        /// Изменился ли формат поверхности — формат картинок ИЛИ цветовое пространство дисплея: тогда пайплайн
        /// рисования надо создать заново (UI зашивает основные цвета экрана в конвейер картинок, 14g).
        format_changed: bool,
    },
    /// Пересоздавать пока нечего: старый swapchain остаётся «устаревшим» (`needs_recreate()`), повторите позже.
    Deferred(ZeroExtentReason),
}

/// Снимок параметров цели (для диагностики, smoke и выбора пайплайна).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayTargetInfo {
    /// Формат картинок swapchain.
    pub format: vk::Format,
    /// Цветовое пространство.
    pub color_space: vk::ColorSpaceKHR,
    /// То же в нейтральном виде (ISP выбирает по нему охват выхода).
    pub display_color_space: super::config::DisplayColorSpace,
    /// Физический размер картинок (с учётом поворота).
    pub extent: vk::Extent2D,
    /// Размер в прямой ориентации (как видит пользователь).
    pub logical_extent: vk::Extent2D,
    /// Поворот дисплея (`preTransform`).
    pub transform: SurfaceTransform,
    /// Число картинок.
    pub image_count: usize,
    /// Режим composite alpha.
    pub composite_alpha: vk::CompositeAlphaFlagsKHR,
    /// Режим показа.
    pub present_mode: vk::PresentModeKHR,
    /// Число кадров в полёте.
    pub frames_in_flight: usize,
    /// Сколько раз swapchain пересоздавали с момента `attach`.
    pub recreation_count: u64,
    /// Доступен ли `VK_GOOGLE_display_timing`.
    pub display_timing_supported: bool,
}

/// `attach` не удался: ошибка и поверхность, которую вызывающий получает обратно.
#[derive(Debug, thiserror::Error)]
#[error("attach не удался: {error}")]
pub struct AttachError {
    /// Причина.
    #[source]
    pub error: SwapchainError,
    /// Поверхность возвращена владельцу нетронутой (Vulkan-объекты, созданные по пути, уже уничтожены).
    pub surface: PresentationSurface,
}

/// `detach` не удался: GPU не освободился вовремя. Vulkan-объекты намеренно оставлены (утечка),
/// поверхность возвращена, но к ней может быть всё ещё подключён живой swapchain: используйте с осторожностью.
#[derive(Debug, thiserror::Error)]
#[error("detach не удался: {error}")]
pub struct DetachError {
    /// Причина (обычно `Timeout`).
    #[source]
    pub error: GpuError,
    /// Поверхность, возвращённая владельцу.
    pub surface: PresentationSurface,
}

/// Цель показа. Подробности — в документации модуля.
pub struct DisplayTarget {
    /// `None` только внутри `detach`/`Drop` (после снятия состояния).
    state: Option<TargetState>,
}

impl std::fmt::Debug for DisplayTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DisplayTarget")
            .field("attached", &self.state.is_some())
            .finish_non_exhaustive()
    }
}

impl DisplayTarget {
    /// Создаёт цель: `VkSurfaceKHR` поверх поверхности, swapchain и кольцо кадров.
    pub fn attach(
        context: &GpuContext,
        surface: PresentationSurface,
        config: DisplayTargetConfig,
    ) -> Result<Self, AttachError> {
        match TargetState::build_parts(context, &surface, &config) {
            Ok(parts) => {
                let state = TargetState::assemble(context, surface, config, parts);
                tracing::info!(
                    target: "fasticamera::gpu::swapchain",
                    info = ?state.info(),
                    "DisplayTarget подключён"
                );
                Ok(Self { state: Some(state) })
            }
            // Все созданные по пути объекты уже уничтожены RAII; окно возвращаем владельцу.
            Err(error) => Err(AttachError { error, surface }),
        }
    }

    fn state(&self) -> Result<&TargetState, SwapchainError> {
        self.state.as_ref().ok_or_else(detached_error)
    }

    fn state_mut(&mut self) -> Result<&mut TargetState, SwapchainError> {
        self.state.as_mut().ok_or_else(detached_error)
    }

    /// Параметры цели.
    pub fn info(&self) -> Result<DisplayTargetInfo, SwapchainError> {
        Ok(self.state()?.info())
    }

    /// Надо ли вызвать `recreate` (устаревший swapchain или неудачное прошлое пересоздание).
    pub fn needs_recreate(&self) -> Result<bool, SwapchainError> {
        Ok(self.state()?.needs_recreate())
    }

    /// Выдаёт следующий кадр на рисование. Не блокируется дольше `acquire_timeout`.
    pub fn acquire_frame(&mut self) -> Result<AcquireOutcome, SwapchainError> {
        self.state_mut()?.acquire_frame()
    }

    /// Показывает кадр.
    pub fn present(&mut self, frame: DisplayFrame, request: PresentRequest) -> Result<PresentOutcome, SwapchainError> {
        self.present_with_sync(frame, request, &PresentSync::none()).outcome
    }

    /// Показывает кадр, добавляя к сабмиту ожидание acquire-fence камеры и/или экспорт release-fence
    /// (см. [`PresentSync`]). Без дополнительной синхронизации ведёт себя в точности как [`Self::present`].
    ///
    /// Возвращает исход показа вместе с токеном экспорта и фактом принятия сабмита: даже если показ
    /// вернул ошибку, принятый сабмит уже запущен на GPU, и вызывающий обязан это учесть.
    pub fn present_with_sync<'sync>(
        &mut self,
        frame: DisplayFrame,
        request: PresentRequest,
        sync: &PresentSync<'sync>,
    ) -> SyncedPresent<'sync> {
        match self.state_mut() {
            Ok(state) => state.present(frame, request, sync),
            Err(error) => SyncedPresent::failed_before_submit(error),
        }
    }

    /// Завершает кадр, который не удалось дорисовать: картинка с неопределённым содержимым всё равно
    /// показывается (иначе семафор слота остался бы сигнальным). Вызывайте вместо `present` при ошибке рисования.
    pub fn abandon_frame(&mut self, frame: DisplayFrame) -> Result<PresentOutcome, SwapchainError> {
        self.present_with_sync(frame, PresentRequest::default(), &PresentSync::none())
            .outcome
    }

    /// Отказывается от кадра, выданного `acquire_frame`, но не рисованного и не показываемого (поток рендера
    /// держал его про запас). В отличие от [`Self::abandon_frame`] НИЧЕГО не показывает: на экран не попадёт
    /// картинка с неопределённым содержимым. Цена — swapchain помечается устаревшим (`needs_recreate`):
    /// выданную картинку без `VK_EXT_swapchain_maintenance1` вернуть нельзя, поэтому её swapchain отбрасывается
    /// (см. `TargetState::discard_unpresented_frame`). Вызывать перед пересозданием и отсоединением цели.
    ///
    /// Кадр помечается завершённым при любом исходе (в том числе у отсоединённой цели): его `Drop` не ругается.
    pub fn discard_unpresented_frame(&mut self, mut frame: DisplayFrame) -> Result<(), SwapchainError> {
        match self.state_mut() {
            Ok(state) => state.discard_unpresented_frame(&mut frame),
            Err(error) => {
                frame.finished = true;
                Err(error)
            }
        }
    }

    /// Пересоздаёт swapchain; `size` — новый размер из `surfaceChanged` (`None` — оставить прежний).
    pub fn recreate(&mut self, size: Option<SurfaceSize>) -> Result<RecreateOutcome, SwapchainError> {
        self.state_mut()?.recreate(size)
    }

    /// Период обновления дисплея; `None` — `VK_GOOGLE_display_timing` недоступно или драйвер не сообщил.
    pub fn refresh_cycle(&self) -> Result<Option<Duration>, SwapchainError> {
        self.state()?.refresh_cycle()
    }

    /// Забирает журнал прошлых показов (пусто без `VK_GOOGLE_display_timing`).
    pub fn drain_past_presentations(&self) -> Result<Vec<PastPresentation>, SwapchainError> {
        self.state()?.drain_past_presentations()
    }

    /// Ждёт только сабмиты цели, не весь GPU: после этого безопасно уничтожать то, на что ссылаются
    /// записанные кадры (например, пайплайн рисования). Ограничено `teardown_timeout`.
    pub fn wait_for_submitted_frames(&self) -> Result<(), SwapchainError> {
        Ok(self.state()?.wait_for_submitted_frames()?)
    }

    /// Отслеживаются ли показы fence'ами (`VK_EXT_swapchain_maintenance1`). Если нет, освобождение
    /// цели при необходимости ждёт `vkDeviceWaitIdle` без таймаута.
    pub fn tracks_presents_with_fences(&self) -> Result<bool, SwapchainError> {
        Ok(self.state()?.tracks_presents_with_fences())
    }

    /// Сколько показов ждут сигнала fence'а (для тестов учёта).
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn pending_present_count(&self) -> Result<usize, SwapchainError> {
        Ok(self.state()?.pending_present_count())
    }

    /// Текущий swapchain уже retired (передан как `oldSwapchain`) — тесты порядка пересоздания.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn current_swapchain_is_retired(&self) -> Result<bool, SwapchainError> {
        Ok(self.state()?.current_swapchain_is_retired())
    }

    /// Взводит одноразовый тестовый шов отказа (см. [`InjectedFault`]).
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn inject_fault(&mut self, fault: super::InjectedFault) -> Result<(), SwapchainError> {
        self.state_mut()?.inject_fault(fault);
        Ok(())
    }

    /// Отсоединяет цель: ждёт GPU, уничтожает swapchain, затем `VkSurfaceKHR`, возвращает поверхность.
    pub fn detach(mut self) -> Result<PresentationSurface, DetachError> {
        let Some(state) = self.state.take() else {
            // Недостижимо через публичный API (detach потребляет self), но не паникуем.
            return Err(DetachError {
                error: GpuError::InvalidState {
                    what: "DisplayTarget уже отсоединён".to_owned(),
                },
                surface: PresentationSurface::Headless,
            });
        };
        state
            .release()
            .map_err(|(error, surface)| DetachError { error, surface })
    }
}

fn detached_error() -> SwapchainError {
    GpuError::InvalidState {
        what: "DisplayTarget уже отсоединён".to_owned(),
    }
    .into()
}

impl Drop for DisplayTarget {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else { return };
        tracing::warn!(
            target: "fasticamera::gpu::swapchain",
            "DisplayTarget уничтожен без detach: освобождаем в том же порядке, поверхность будет уничтожена вместе с ним"
        );
        if let Err((error, _surface)) = state.release() {
            tracing::error!(target: "fasticamera::gpu::swapchain", %error, "освобождение DisplayTarget в Drop не удалось");
        }
    }
}
