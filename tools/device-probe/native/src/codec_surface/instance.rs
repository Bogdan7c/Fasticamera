//! Выделенные Vulkan instance + surface + device для теста. Общий контекст пробы
//! (`vulkan::context`) не трогаем: у него нет WSI-расширений, и ломать его нельзя.

use std::ffi::{CStr, c_char};

use ash::{Device, Entry, Instance, khr, vk};

use super::window::NativeWindow;
use crate::vulkan::context::{list_device_extensions, load_entry};
use crate::vulkan::vk_step;

const EXT_SWAPCHAIN_COLORSPACE: &CStr = c"VK_EXT_swapchain_colorspace";
pub const GOOGLE_DISPLAY_TIMING: &CStr = c"VK_GOOGLE_display_timing";

/// Instance + `VkSurfaceKHR` поверх `ANativeWindow`. `Drop` уничтожает surface, затем instance.
pub struct SurfaceInstance {
    _entry: Entry,
    pub instance: Instance,
    pub surface_loader: khr::surface::Instance,
    pub surface: vk::SurfaceKHR,
    /// Включено ли `VK_EXT_swapchain_colorspace` (даёт HDR-цветовые пространства).
    pub has_colorspace_ext: bool,
}

/// Логическое устройство с очередью, умеющей и графику, и показ на surface.
pub struct PresentDevice {
    pub device: Device,
    pub physical_device: vk::PhysicalDevice,
    pub queue: vk::Queue,
    pub queue_family_index: u32,
    pub device_name: String,
    /// Включено ли `VK_GOOGLE_display_timing` (desiredPresentTime → PTS кодека).
    pub has_display_timing: bool,
    pub extensions: Vec<String>,
}

fn has_name(available: &[String], name: &CStr) -> bool {
    available
        .iter()
        .any(|candidate| candidate.as_str() == name.to_string_lossy())
}

impl SurfaceInstance {
    /// Создаёт instance (API 1.1) с WSI-расширениями и surface поверх окна.
    pub fn create(window: &NativeWindow) -> Result<Self, String> {
        let entry = load_entry()?;
        // SAFETY: entry валиден; None — расширения самого loader'а.
        let available: Vec<String> = vk_step(
            unsafe { entry.enumerate_instance_extension_properties(None) },
            "vkEnumerateInstanceExtensionProperties",
        )?
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();

        let mut names: Vec<*const c_char> = Vec::new();
        for required in [khr::surface::NAME, khr::android_surface::NAME] {
            if !has_name(&available, required) {
                return Err(format!("instance не поддерживает {}", required.to_string_lossy()));
            }
            names.push(required.as_ptr());
        }
        let has_colorspace_ext = has_name(&available, EXT_SWAPCHAIN_COLORSPACE);
        if has_colorspace_ext {
            names.push(EXT_SWAPCHAIN_COLORSPACE.as_ptr());
        }

        let application = vk::ApplicationInfo::default()
            .application_name(c"FasticameraProbeCodecSurface")
            .api_version(vk::make_api_version(0, 1, 1, 0));
        let create_info = vk::InstanceCreateInfo::default()
            .application_info(&application)
            .enabled_extension_names(&names);
        // SAFETY: create_info ссылается на живые локальные данные.
        let instance = vk_step(unsafe { entry.create_instance(&create_info, None) }, "vkCreateInstance")?;

        let surface_loader = khr::surface::Instance::new(&entry, &instance);
        let android_loader = khr::android_surface::Instance::new(&entry, &instance);
        let surface_info = vk::AndroidSurfaceCreateInfoKHR::default().window(window.as_ptr().cast());
        // SAFETY: окно живо (его держит вызывающий дольше, чем surface).
        match vk_step(
            unsafe { android_loader.create_android_surface(&surface_info, None) },
            "vkCreateAndroidSurfaceKHR",
        ) {
            Ok(surface) => Ok(Self {
                _entry: entry,
                instance,
                surface_loader,
                surface,
                has_colorspace_ext,
            }),
            Err(error) => {
                // SAFETY: surface не создан, instance больше никто не использует.
                unsafe { instance.destroy_instance(None) };
                Err(error)
            }
        }
    }

    /// Выбирает GPU с `VK_KHR_swapchain` и очередью graphics+present, создаёт устройство.
    pub fn create_device(&self) -> Result<PresentDevice, String> {
        // SAFETY: instance валиден.
        let physicals = vk_step(
            unsafe { self.instance.enumerate_physical_devices() },
            "vkEnumeratePhysicalDevices",
        )?;
        for physical in physicals {
            let extensions = list_device_extensions(&self.instance, physical);
            if !has_name(&extensions, khr::swapchain::NAME) {
                continue;
            }
            if let Some(family) = self.find_present_family(physical)? {
                return self.build_device(physical, family, extensions);
            }
        }
        Err("нет GPU с VK_KHR_swapchain и очередью graphics+present для этой surface".to_owned())
    }

    fn find_present_family(&self, physical: vk::PhysicalDevice) -> Result<Option<u32>, String> {
        // SAFETY: physical получен из этого instance.
        let families = unsafe { self.instance.get_physical_device_queue_family_properties(physical) };
        for (index, family) in families.iter().enumerate() {
            let index = index as u32;
            // SAFETY: индекс семейства из списка этого устройства; surface жива.
            let present = vk_step(
                unsafe {
                    self.surface_loader
                        .get_physical_device_surface_support(physical, index, self.surface)
                },
                "vkGetPhysicalDeviceSurfaceSupportKHR",
            )?;
            if present && family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn build_device(
        &self,
        physical: vk::PhysicalDevice,
        family: u32,
        extensions: Vec<String>,
    ) -> Result<PresentDevice, String> {
        let has_display_timing = has_name(&extensions, GOOGLE_DISPLAY_TIMING);
        let mut names: Vec<*const c_char> = vec![khr::swapchain::NAME.as_ptr()];
        if has_display_timing {
            names.push(GOOGLE_DISPLAY_TIMING.as_ptr());
        }
        let priorities = [1.0_f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&names);
        // SAFETY: все указатели в create_info живут до конца вызова.
        let device = vk_step(
            unsafe { self.instance.create_device(physical, &create_info, None) },
            "vkCreateDevice",
        )?;
        // SAFETY: device создан с очередью этого семейства (индекс 0).
        let queue = unsafe { device.get_device_queue(family, 0) };
        // SAFETY: physical валиден.
        let properties = unsafe { self.instance.get_physical_device_properties(physical) };
        let device_name = properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "<неизвестно>".to_owned());
        Ok(PresentDevice {
            device,
            physical_device: physical,
            queue,
            queue_family_index: family,
            device_name,
            has_display_timing,
            extensions,
        })
    }
}

impl Drop for SurfaceInstance {
    fn drop(&mut self) {
        // SAFETY: устройство и swapchain уничтожены раньше (порядок полей Session), surface и instance наши.
        unsafe {
            self.surface_loader.destroy_surface(self.surface, None);
            self.instance.destroy_instance(None);
        }
    }
}

impl PresentDevice {
    /// Ждёт простоя устройства: ошибка попадает в отчёт teardown, а не теряется.
    pub fn shutdown(&self) -> Result<(), String> {
        // SAFETY: device валиден до Drop.
        vk_step(unsafe { self.device.device_wait_idle() }, "vkDeviceWaitIdle")
    }
}

impl Drop for PresentDevice {
    fn drop(&mut self) {
        // SAFETY: объекты на устройстве уничтожены раньше (Rig), очередь простаивает.
        unsafe { self.device.destroy_device(None) };
    }
}
