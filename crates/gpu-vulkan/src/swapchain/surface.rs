//! `PresentationSurface` — что именно показываем, создание `VkSurfaceKHR` и запрос возможностей.
//!
//! Владение: [`PresentationSurface`] владеет окном платформы (на Android — `NativeSurface`),
//! `OwnedSurface` владеет `VkSurfaceKHR`, созданным поверх него. Окно должно жить дольше
//! `VkSurfaceKHR` — поэтому цель хранит оба и возвращает окно только после уничтожения `VkSurfaceKHR`.

use ash::vk;
#[cfg(target_os = "android")]
use shell_contract::surface::NativeSurface;

use super::{SwapchainError, swapchain_step};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::live_objects::OwnedSurface;

/// Поверхность показа.
#[derive(Debug)]
pub enum PresentationSurface {
    /// Окно Android (`ANativeWindow` внутри `NativeSurface`); `VkSurfaceKHR` создаётся через
    /// `VK_KHR_android_surface`.
    #[cfg(target_os = "android")]
    Android(NativeSurface),
    /// Стенд ПК без окна (`VK_EXT_headless_surface`): картинки «показываются» в никуда.
    /// Нужен, чтобы гонять swapchain-код и validation на настоящем драйвере без экрана.
    Headless,
}

impl PresentationSurface {
    /// Создаёт `VkSurfaceKHR` поверх этой поверхности.
    ///
    /// Профиль контекста должен включать нужное расширение instance, иначе — `ExtensionNotEnabled`.
    pub(crate) fn create_vulkan_surface(&self, context: &GpuContext) -> Result<OwnedSurface, SwapchainError> {
        let raw_surface = match self {
            #[cfg(target_os = "android")]
            PresentationSurface::Android(native) => {
                let loader = context.android_surface_loader()?;
                let create_info = vk::AndroidSurfaceCreateInfoKHR::default().window(native.raw_window().as_ptr());
                // SAFETY: окно живо, пока жив `native` (мы владеем им в `self`); create_info — живая локальная
                // структура; instance — тот, на котором включено расширение.
                swapchain_step(
                    unsafe { loader.create_android_surface(&create_info, None) },
                    "vkCreateAndroidSurfaceKHR",
                )?
            }
            PresentationSurface::Headless => {
                let loader = context.headless_surface_loader()?;
                let create_info = vk::HeadlessSurfaceCreateInfoEXT::default();
                // SAFETY: create_info — живая локальная структура; расширение включено профилем.
                swapchain_step(
                    unsafe { loader.create_headless_surface(&create_info, None) },
                    "vkCreateHeadlessSurfaceEXT",
                )?
            }
        };
        // Сразу берём во владение: при любой ошибке ниже surface уничтожится RAII.
        Ok(OwnedSurface::from_raw(context, raw_surface)?)
    }
}

/// Живой снимок возможностей surface: возможности и список форматов.
pub(crate) struct SurfaceQuery {
    pub(crate) capabilities: vk::SurfaceCapabilitiesKHR,
    pub(crate) formats: Vec<vk::SurfaceFormatKHR>,
}

/// Спрашивает у драйвера возможности surface и проверяет, что очередь контекста умеет показывать на ней.
pub(crate) fn query_surface(context: &GpuContext, surface: &OwnedSurface) -> Result<SurfaceQuery, SwapchainError> {
    let loader = context.surface_loader()?;
    let physical_device = context.physical_device();
    // SAFETY: physical_device и surface принадлежат одному instance; обе сущности живы (владеем).
    let present_supported = swapchain_step(
        unsafe {
            loader.get_physical_device_surface_support(physical_device, context.queue_family_index(), surface.handle())
        },
        "vkGetPhysicalDeviceSurfaceSupportKHR",
    )?;
    if !present_supported {
        return Err(GpuError::Unsupported {
            what: "очередь контекста не умеет показывать на этой surface".to_owned(),
        }
        .into());
    }
    // SAFETY: см. выше.
    let capabilities = swapchain_step(
        unsafe { loader.get_physical_device_surface_capabilities(physical_device, surface.handle()) },
        "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
    )?;
    // SAFETY: см. выше.
    let formats = swapchain_step(
        unsafe { loader.get_physical_device_surface_formats(physical_device, surface.handle()) },
        "vkGetPhysicalDeviceSurfaceFormatsKHR",
    )?;
    Ok(SurfaceQuery { capabilities, formats })
}
