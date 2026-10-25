//! Опрос возможностей surface кодека: поддержка, capabilities, форматы, режимы показа.

use ash::vk;
use serde_json::json;

use super::instance::{PresentDevice, SurfaceInstance};
use super::policy::{COLOR_SPACE_HDR10_HLG, COLOR_SPACE_HDR10_ST2084, RawSurfaceFormat, is_ten_bit_format};
use super::report::{FormatEntry, SurfaceReport};
use crate::vulkan::vk_step;

/// Результат опроса: отчёт + данные, нужные для создания swapchain.
pub struct SurfaceQuery {
    pub report: SurfaceReport,
    pub capabilities: vk::SurfaceCapabilitiesKHR,
    pub formats: Vec<vk::SurfaceFormatKHR>,
}

/// Переводит пару Vulkan в «сырой» вид для чистой политики выбора.
pub fn to_raw(format: &vk::SurfaceFormatKHR) -> RawSurfaceFormat {
    RawSurfaceFormat {
        format: format.format.as_raw(),
        color_space: format.color_space.as_raw(),
    }
}

/// Выполняет все четыре запроса `vkGetPhysicalDeviceSurface*`; первая ошибка прерывает опрос.
pub fn query_surface(host: &SurfaceInstance, device: &PresentDevice) -> Result<SurfaceQuery, String> {
    let (loader, physical, surface) = (&host.surface_loader, device.physical_device, host.surface);
    // SAFETY: physical и surface принадлежат этому instance и живы.
    let (supported, capabilities, formats, present_modes) = unsafe {
        (
            vk_step(
                loader.get_physical_device_surface_support(physical, device.queue_family_index, surface),
                "vkGetPhysicalDeviceSurfaceSupportKHR",
            )?,
            vk_step(
                loader.get_physical_device_surface_capabilities(physical, surface),
                "vkGetPhysicalDeviceSurfaceCapabilitiesKHR",
            )?,
            vk_step(
                loader.get_physical_device_surface_formats(physical, surface),
                "vkGetPhysicalDeviceSurfaceFormatsKHR",
            )?,
            vk_step(
                loader.get_physical_device_surface_present_modes(physical, surface),
                "vkGetPhysicalDeviceSurfacePresentModesKHR",
            )?,
        )
    };
    let report = SurfaceReport {
        supported,
        capabilities: capabilities_json(&capabilities),
        formats: formats.iter().map(format_entry).collect(),
        present_modes: present_modes.iter().map(|mode| format!("{mode:?}")).collect(),
        has_ten_bit_format: formats.iter().any(|f| is_ten_bit_format(f.format.as_raw())),
        has_hdr10_color_space: formats
            .iter()
            .any(|f| matches!(f.color_space.as_raw(), COLOR_SPACE_HDR10_ST2084 | COLOR_SPACE_HDR10_HLG)),
    };
    Ok(SurfaceQuery {
        report,
        capabilities,
        formats,
    })
}

/// Текстовое представление пары формат + цветовое пространство.
pub fn format_entry(format: &vk::SurfaceFormatKHR) -> FormatEntry {
    FormatEntry {
        format: format!("{:?}", format.format),
        color_space: format!("{:?}", format.color_space),
    }
}

fn extent_json(extent: vk::Extent2D) -> [u32; 2] {
    [extent.width, extent.height]
}

fn capabilities_json(caps: &vk::SurfaceCapabilitiesKHR) -> serde_json::Value {
    json!({
        "min_image_count": caps.min_image_count,
        "max_image_count": caps.max_image_count,
        "current_extent": extent_json(caps.current_extent),
        "min_image_extent": extent_json(caps.min_image_extent),
        "max_image_extent": extent_json(caps.max_image_extent),
        "max_image_array_layers": caps.max_image_array_layers,
        "supported_transforms": format!("{:?}", caps.supported_transforms),
        "current_transform": format!("{:?}", caps.current_transform),
        "supported_composite_alpha": format!("{:?}", caps.supported_composite_alpha),
        "supported_usage_flags": format!("{:?}", caps.supported_usage_flags),
    })
}
