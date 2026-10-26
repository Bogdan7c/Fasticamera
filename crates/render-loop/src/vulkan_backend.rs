//! Настоящий бэкенд показа поверх `gpu-vulkan`: `DisplayTarget` + тестовый паттерн.
//!
//! Архитектура (пакет C1, сессия 03):
//! * [`VulkanDisplayBackend`] реализует контракт [`DisplayBackend`]; поток рендера видит
//!   только этот трейт и нейтральные типы `backend`, а типы Vulkan переводятся здесь (`mapping`);
//! * `session::AttachedDisplay` владеет живым показом (цель, паттерн, окно ПК-стенда) и
//!   порядком уничтожения; бэкенд держит `Option` такого показа;
//! * `config` — настройки; `present_history` — мост «id показа Vulkan -> кадр потока».
//!
//! Режимы показа: [`VulkanDisplayBackend::new_android`] — боевой (Android, окно оболочки);
//! `new_headless_presentation` — СТЕНД ТОЛЬКО ДЛЯ ПК: swapchain на `VK_EXT_headless_surface`.
//! Настоящее окно там не используется, но `NativeSurface` из вызова `attach` бэкенд удерживает
//! и возвращает в `detach`, чтобы протокол «окно отпущено -> квитанция» проверялся одинаково.

mod census;
mod config;
mod mapping;
mod present_history;
mod present_time;
mod session;

pub use config::VulkanBackendConfig;

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::error::GpuInitError;
use gpu_vulkan::swapchain::PresentationSurface;
use shell_contract::surface::{NativeSurface, SurfaceSize};

use crate::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameTiming, PastPresentation, PresentOutcome,
};
use census::census_by_kind;
use session::AttachedDisplay;

const LOG_TARGET: &str = "fasticamera::render::vulkan";

/// Как окно из `attach` превращается в поверхность показа.
#[derive(Debug, Clone, Copy)]
enum PresentationMode {
    /// Боевой режим: окно Android передаётся цели во владение.
    #[cfg(target_os = "android")]
    AndroidWindow,
    /// Стенд ПК: цель работает на headless-surface, а окно хранит сам бэкенд.
    #[cfg(not(target_os = "android"))]
    HeadlessBench,
}

impl PresentationMode {
    /// Возвращает поверхность для цели и окно, которое бэкенд должен удержать сам.
    fn wrap_window(self, window: NativeSurface) -> (PresentationSurface, Option<NativeSurface>) {
        match self {
            #[cfg(target_os = "android")]
            PresentationMode::AndroidWindow => (PresentationSurface::Android(window), None),
            #[cfg(not(target_os = "android"))]
            PresentationMode::HeadlessBench => (PresentationSurface::Headless, Some(window)),
        }
    }
}

/// Бэкенд показа на Vulkan. Все методы вызываются из потока рендера.
pub struct VulkanDisplayBackend {
    context: GpuContext,
    config: VulkanBackendConfig,
    mode: PresentationMode,
    /// `None` — surface не присоединена.
    attached: Option<AttachedDisplay>,
    /// Сколько раз `detach` завершился ошибкой (в диагностику).
    detach_failures: u64,
}

impl VulkanDisplayBackend {
    /// Боевой конструктор для Android: профиль `AndroidDisplay` (swapchain, AHB, display timing).
    /// Ошибка — честный `GpuInitError` (нет Vulkan, расширения, слоя validation…).
    #[cfg(target_os = "android")]
    pub fn new_android(config: VulkanBackendConfig) -> Result<Self, GpuInitError> {
        Self::with_profile(config, DeviceProfile::AndroidDisplay, PresentationMode::AndroidWindow)
    }

    /// Конструктор СТЕНДА ПК: профиль `HeadlessPresentation` (swapchain на
    /// `VK_EXT_headless_surface`, без экрана и без display timing). Существует только для
    /// ПК-бенча и тестов потока рендера на настоящем драйвере; на телефоне не используется.
    #[cfg(not(target_os = "android"))]
    pub fn new_headless_presentation(config: VulkanBackendConfig) -> Result<Self, GpuInitError> {
        Self::with_profile(
            config,
            DeviceProfile::HeadlessPresentation,
            PresentationMode::HeadlessBench,
        )
    }

    fn with_profile(
        config: VulkanBackendConfig,
        profile: DeviceProfile,
        mode: PresentationMode,
    ) -> Result<Self, GpuInitError> {
        let context = GpuContext::new(&GpuContextConfig::new(profile, config.validation.clone()))?;
        Ok(Self {
            context,
            config,
            mode,
            attached: None,
            detach_failures: 0,
        })
    }

    /// Взводит одноразовый тестовый шов отказа в присоединённой цели (feature `fault-injection`):
    /// например, изобразить зависший GPU, чтобы пройти путь «detach не удался» детерминированно.
    #[cfg(feature = "fault-injection")]
    pub fn inject_fault(&mut self, fault: gpu_vulkan::swapchain::InjectedFault) -> Result<(), BackendError> {
        match self.attached.as_mut() {
            Some(display) => display.inject_fault(fault),
            None => Err(BackendError {
                operation: "inject_fault".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    /// Сколько показов текущей цепочки ждут записи display timing (feature `fault-injection`,
    /// тестовый шов); `None` — surface не присоединена.
    #[cfg(feature = "fault-injection")]
    pub fn present_history_len(&self) -> Option<usize> {
        self.attached.as_ref().map(AttachedDisplay::present_history_len)
    }

    /// Контекст GPU бэкенда (дёшево клонируется). Нужен снаружи, чтобы до запуска потока
    /// снять базовую линию живых объектов и после — проверить, что они вернулись.
    pub fn context(&self) -> &GpuContext {
        &self.context
    }
}

impl DisplayBackend for VulkanDisplayBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        if self.attached.is_some() {
            // Поток обязан сначала отсоединить прежнюю surface; новая уничтожается здесь.
            return Err(AttachError::SurfaceRejected(BackendError {
                operation: "attach".to_owned(),
                reason: "бэкенд уже держит surface: сначала detach".to_owned(),
            }));
        }
        let (presentation, held_surface) = self.mode.wrap_window(surface);
        let display = AttachedDisplay::attach(&self.context, &self.config, presentation, held_surface, size)?;
        self.attached = Some(display);
        Ok(())
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        match self.attached.as_mut() {
            Some(display) => display.resize(size),
            None => Err(BackendError {
                operation: "resize".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        match self.attached.as_mut() {
            Some(display) => display.render_and_present(timing),
            None => PresentOutcome::Failed(BackendError {
                operation: "render_and_present".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        let display = self.attached.take()?;
        let report = display.release();
        if let Some(error) = &report.error {
            // Окно возвращаем всё равно (иначе оболочка ждала бы таймаут), но ошибку не прячем.
            self.detach_failures += 1;
            tracing::error!(
                target: LOG_TARGET,
                %error,
                "detach завершился ошибкой: окно возвращено оболочке и квитанция будет подтверждена, но VkSurfaceKHR/swapchain/кольцо кадров/пайплайн утекли; утёкшая VkSurfaceKHR может удерживать подключение производителя к окну"
            );
        }
        report.surface
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        let validation = self.context.validation_report();
        let snapshot = self.context.live_objects();
        let live_objects = u64::try_from(snapshot.total()).unwrap_or(u64::MAX);
        let live_objects_by_kind = census_by_kind(&snapshot);
        let display = self.attached.as_ref().and_then(AttachedDisplay::display_info);
        BackendDiagnostics {
            live_objects,
            live_objects_by_kind,
            validation_errors: validation.errors,
            validation_warnings: validation.warnings,
            detach_failures: self.detach_failures,
            display,
            known_deviations: validation.known_deviations,
        }
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        match self.attached.as_mut() {
            Some(display) => display.drain_past_presentations(),
            None => Vec::new(),
        }
    }
}
