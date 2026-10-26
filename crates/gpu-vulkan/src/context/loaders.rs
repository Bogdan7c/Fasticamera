//! Загрузчики функций расширений. Создаются только для расширений, которые реально включены.
//!
//! Доступ — через методы, возвращающие `Result`: «расширение не включено профилем» — это
//! типизированная ошибка `GpuError::ExtensionNotEnabled`, а не `Option`, о котором можно забыть.

use std::ffi::CStr;

use ash::{Device, Entry, Instance, ext, google, khr};

use super::GpuContext;
use crate::error::GpuError;

/// Загрузчики расширений instance и device, включённых профилем.
pub struct ExtensionLoaders {
    surface: Option<khr::surface::Instance>,
    #[cfg_attr(
        all(not(target_os = "android"), not(test)),
        expect(
            dead_code,
            reason = "VkSurfaceKHR из ANativeWindow создаётся только на Android (и в unit-тестах контекста)"
        )
    )]
    android_surface: Option<khr::android_surface::Instance>,
    headless_surface: Option<ext::headless_surface::Instance>,
    swapchain: Option<khr::swapchain::Device>,
    swapchain_maintenance1: Option<ext::swapchain_maintenance1::Device>,
    external_semaphore_fd: Option<khr::external_semaphore_fd::Device>,
    external_fence_fd: Option<khr::external_fence_fd::Device>,
    display_timing: Option<google::display_timing::Device>,
    #[cfg_attr(
        not(target_os = "android"),
        expect(dead_code, reason = "импорт AHardwareBuffer есть только на Android")
    )]
    android_hardware_buffer: Option<ash::android::external_memory_android_hardware_buffer::Device>,
}

fn enabled(list: &[&CStr], name: &CStr) -> bool {
    list.contains(&name)
}

impl ExtensionLoaders {
    /// Создаёт загрузчики по спискам включённых расширений.
    pub(super) fn new(
        entry: &Entry,
        instance: &Instance,
        device: &Device,
        instance_extensions: &[&CStr],
        device_extensions: &[&CStr],
    ) -> Self {
        Self {
            surface: enabled(instance_extensions, khr::surface::NAME)
                .then(|| khr::surface::Instance::new(entry, instance)),
            android_surface: enabled(instance_extensions, khr::android_surface::NAME)
                .then(|| khr::android_surface::Instance::new(entry, instance)),
            headless_surface: enabled(instance_extensions, ext::headless_surface::NAME)
                .then(|| ext::headless_surface::Instance::new(entry, instance)),
            swapchain: enabled(device_extensions, khr::swapchain::NAME)
                .then(|| khr::swapchain::Device::new(instance, device)),
            swapchain_maintenance1: enabled(device_extensions, ext::swapchain_maintenance1::NAME)
                .then(|| ext::swapchain_maintenance1::Device::new(instance, device)),
            external_semaphore_fd: enabled(device_extensions, khr::external_semaphore_fd::NAME)
                .then(|| khr::external_semaphore_fd::Device::new(instance, device)),
            external_fence_fd: enabled(device_extensions, khr::external_fence_fd::NAME)
                .then(|| khr::external_fence_fd::Device::new(instance, device)),
            display_timing: enabled(device_extensions, google::display_timing::NAME)
                .then(|| google::display_timing::Device::new(instance, device)),
            android_hardware_buffer: enabled(
                device_extensions,
                ash::android::external_memory_android_hardware_buffer::NAME,
            )
            .then(|| ash::android::external_memory_android_hardware_buffer::Device::new(instance, device)),
        }
    }
}

/// Превращает `Option` загрузчика в ошибку с именем расширения.
fn require<'a, T>(loader: &'a Option<T>, name: &'static CStr) -> Result<&'a T, GpuError> {
    loader.as_ref().ok_or(GpuError::ExtensionNotEnabled {
        name: name.to_str().unwrap_or("<не UTF-8>"),
    })
}

impl GpuContext {
    /// `VK_KHR_surface` (instance).
    pub(crate) fn surface_loader(&self) -> Result<&khr::surface::Instance, GpuError> {
        require(&self.loaders().surface, khr::surface::NAME)
    }

    /// `VK_KHR_android_surface` (instance).
    #[cfg_attr(
        all(not(target_os = "android"), not(test)),
        expect(
            dead_code,
            reason = "VkSurfaceKHR из ANativeWindow создаётся только на Android (и в unit-тестах контекста)"
        )
    )]
    pub(crate) fn android_surface_loader(&self) -> Result<&khr::android_surface::Instance, GpuError> {
        require(&self.loaders().android_surface, khr::android_surface::NAME)
    }

    /// `VK_EXT_headless_surface` (instance).
    pub(crate) fn headless_surface_loader(&self) -> Result<&ext::headless_surface::Instance, GpuError> {
        require(&self.loaders().headless_surface, ext::headless_surface::NAME)
    }

    /// `VK_KHR_swapchain` (device).
    pub(crate) fn swapchain_loader(&self) -> Result<&khr::swapchain::Device, GpuError> {
        require(&self.loaders().swapchain, khr::swapchain::NAME)
    }

    /// Включено ли `VK_EXT_swapchain_maintenance1`: только с ним у показа бывают fence'ы
    /// (`VkSwapchainPresentFenceInfoEXT`), по которым можно дождаться окончания показа с таймаутом.
    pub(crate) fn present_fences_supported(&self) -> bool {
        self.loaders().swapchain_maintenance1.is_some()
    }

    /// `VK_KHR_external_semaphore_fd` (device).
    pub(crate) fn external_semaphore_fd_loader(&self) -> Result<&khr::external_semaphore_fd::Device, GpuError> {
        require(&self.loaders().external_semaphore_fd, khr::external_semaphore_fd::NAME)
    }

    /// `VK_KHR_external_fence_fd` (device); в профиле `Headless` необязательное.
    pub(crate) fn external_fence_fd_loader(&self) -> Result<&khr::external_fence_fd::Device, GpuError> {
        require(&self.loaders().external_fence_fd, khr::external_fence_fd::NAME)
    }

    /// `VK_GOOGLE_display_timing` (device).
    pub(crate) fn display_timing_loader(&self) -> Result<&google::display_timing::Device, GpuError> {
        require(&self.loaders().display_timing, google::display_timing::NAME)
    }

    /// `VK_ANDROID_external_memory_android_hardware_buffer` (device).
    #[cfg_attr(
        not(target_os = "android"),
        expect(dead_code, reason = "импорт AHardwareBuffer есть только на Android")
    )]
    pub(crate) fn android_hardware_buffer_loader(
        &self,
    ) -> Result<&ash::android::external_memory_android_hardware_buffer::Device, GpuError> {
        require(
            &self.loaders().android_hardware_buffer,
            ash::android::external_memory_android_hardware_buffer::NAME,
        )
    }
}
