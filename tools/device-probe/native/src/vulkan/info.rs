//! Отчёт `vulkanInfo`: что умеет Vulkan-драйвер телефона в части импорта AHB и sync_fd.

use ash::vk;
use serde_json::{Value, json};

use super::context::{self, list_device_extensions, shared_context};
use crate::error::ProbeError;
use crate::report::extension_presence;

/// Расширения, наличие которых нас интересует.
const WATCHED_EXTENSIONS: &[&str] = &[
    "VK_ANDROID_external_memory_android_hardware_buffer",
    "VK_KHR_external_semaphore_fd",
    "VK_KHR_external_fence_fd",
    "VK_KHR_sampler_ycbcr_conversion",
    "VK_EXT_queue_family_foreign",
    "VK_KHR_timeline_semaphore",
    "VK_KHR_synchronization2",
];

/// Форматы, под которые проверяем импорт AHB как образа.
const PROBED_FORMATS: &[vk::Format] = &[vk::Format::R16_UINT, vk::Format::R16_UNORM, vk::Format::R8_UINT];

fn version_string(version: u32) -> String {
    format!(
        "{}.{}.{}",
        vk::api_version_major(version),
        vk::api_version_minor(version),
        vk::api_version_patch(version)
    )
}

/// `vkGetPhysicalDeviceImageFormatProperties2` с внешним типом AHB.
fn probe_ahb_image_format(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
) -> Value {
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::ANDROID_HARDWARE_BUFFER_ANDROID);
    let format_info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .push_next(&mut external_info);
    let mut external_properties = vk::ExternalImageFormatProperties::default();
    let mut properties = vk::ImageFormatProperties2::default().push_next(&mut external_properties);
    // SAFETY: physical получен из instance; структуры локальные и живут до конца вызова.
    let result =
        unsafe { instance.get_physical_device_image_format_properties2(physical, &format_info, &mut properties) };
    match result {
        Ok(()) => {
            let max_extent = properties.image_format_properties.max_extent;
            let memory = external_properties.external_memory_properties;
            json!({
                "format": format!("{format:?}"),
                "usage": format!("{usage:?}"),
                "supported": true,
                "max_extent": [max_extent.width, max_extent.height],
                "external_memory_features": format!("{:?}", memory.external_memory_features),
                "compatible_handle_types": format!("{:?}", memory.compatible_handle_types),
            })
        }
        Err(code) => json!({
            "format": format!("{format:?}"),
            "usage": format!("{usage:?}"),
            "supported": false,
            "error": format!("{code:?}"),
        }),
    }
}

/// Свойства внешнего семафора SYNC_FD.
fn probe_external_semaphore(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Value {
    let info =
        vk::PhysicalDeviceExternalSemaphoreInfo::default().handle_type(vk::ExternalSemaphoreHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalSemaphoreProperties::default();
    // SAFETY: physical валиден, структуры локальные.
    unsafe { instance.get_physical_device_external_semaphore_properties(physical, &info, &mut properties) };
    json!({
        "features": format!("{:?}", properties.external_semaphore_features),
        "compatible_handle_types": format!("{:?}", properties.compatible_handle_types),
    })
}

/// Свойства внешнего fence SYNC_FD.
fn probe_external_fence(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Value {
    let info = vk::PhysicalDeviceExternalFenceInfo::default().handle_type(vk::ExternalFenceHandleTypeFlags::SYNC_FD);
    let mut properties = vk::ExternalFenceProperties::default();
    // SAFETY: physical валиден, структуры локальные.
    unsafe { instance.get_physical_device_external_fence_properties(physical, &info, &mut properties) };
    json!({
        "features": format!("{:?}", properties.external_fence_features),
        "compatible_handle_types": format!("{:?}", properties.compatible_handle_types),
    })
}

/// Описание одного физического устройства.
fn describe_physical_device(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Value {
    // SAFETY: physical получен из этого instance.
    let (properties, families, features) = unsafe {
        (
            instance.get_physical_device_properties(physical),
            instance.get_physical_device_queue_family_properties(physical),
            instance.get_physical_device_features(physical),
        )
    };
    let extensions = list_device_extensions(instance, physical);
    let queue_families: Vec<Value> = families
        .iter()
        .enumerate()
        .map(|(index, family)| {
            json!({
                "index": index,
                "flags": format!("{:?}", family.queue_flags),
                "queue_count": family.queue_count,
                "timestamp_valid_bits": family.timestamp_valid_bits,
            })
        })
        .collect();
    let storage_and_sampled = vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::SAMPLED;
    let mut ahb_image_formats = Vec::new();
    for format in PROBED_FORMATS {
        ahb_image_formats.push(probe_ahb_image_format(instance, physical, *format, storage_and_sampled));
        ahb_image_formats.push(probe_ahb_image_format(
            instance,
            physical,
            *format,
            vk::ImageUsageFlags::SAMPLED,
        ));
    }
    json!({
        "device_name": properties.device_name_as_c_str().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
        "device_type": format!("{:?}", properties.device_type),
        "vendor_id": format!("0x{:X}", properties.vendor_id),
        "device_id": format!("0x{:X}", properties.device_id),
        "api_version": version_string(properties.api_version),
        "driver_version": format!("0x{:X}", properties.driver_version),
        "extensions_of_interest": extension_presence(WATCHED_EXTENSIONS, &extensions),
        "extension_count": extensions.len(),
        "shader_storage_image_extended_formats": features.shader_storage_image_extended_formats == vk::TRUE,
        "queue_families": queue_families,
        "external_semaphore_sync_fd": probe_external_semaphore(instance, physical),
        "external_fence_sync_fd": probe_external_fence(instance, physical),
        "ahb_image_formats": ahb_image_formats,
    })
}

/// Проверка, что рабочий контекст (device + pipelines) реально создаётся.
fn probe_device_creation() -> Value {
    match shared_context() {
        Ok(ctx) => json!({
            "ok": true,
            "device_name": ctx.caps.device_name,
            "timestamp_valid_bits": ctx.caps.timestamp_valid_bits,
            "timestamp_period_ns": ctx.caps.timestamp_period_ns,
            "semaphore_fd_enabled": ctx.semaphore_fd_loader.is_some(),
            "storage_image_extended_formats_enabled": ctx.caps.storage_image_extended_formats,
        }),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

/// Собирает JSON `vulkanInfo`.
pub fn build_vulkan_info_json() -> Result<String, ProbeError> {
    let entry = context::load_entry().map_err(ProbeError::Platform)?;
    // SAFETY: entry валиден.
    let instance_version = unsafe { entry.try_enumerate_instance_version() }
        .ok()
        .flatten()
        .map_or_else(|| "1.0.0".to_owned(), version_string);
    let instance = context::create_instance(&entry).map_err(ProbeError::Platform)?;

    // SAFETY: instance валиден.
    let devices = unsafe { instance.enumerate_physical_devices() };
    let devices_json: Result<Vec<Value>, String> = match devices {
        Ok(devices) => Ok(devices
            .into_iter()
            .map(|physical| describe_physical_device(&instance, physical))
            .collect()),
        Err(code) => Err(format!("vkEnumeratePhysicalDevices: {code:?}")),
    };
    // SAFETY: временный instance не используется после этой точки (объекты Vulkan из него — только JSON).
    unsafe { instance.destroy_instance(None) };

    let report = json!({
        "ok": true,
        "instance_api_version": instance_version,
        "physical_devices": devices_json.map_err(ProbeError::Platform)?,
        "device_creation": probe_device_creation(),
    });
    Ok(serde_json::to_string(&report)?)
}
