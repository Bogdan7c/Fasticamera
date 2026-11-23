//! Чтение отчётов о физических устройствах из настоящего Vulkan.
//!
//! Всё, что решает «подходит ли устройство», живёт в чистом [`super::select_device`];
//! здесь только сбор данных через `vkGet*`.

use ash::{Instance, ext, vk};

use super::{DeviceCandidate, FeatureSupport, QueueFamilyReport};
use crate::context::Float32Controls;
use crate::error::{GpuInitError, init_step};

/// Сведения об устройстве, которые не участвуют в выборе, но нужны отчёту о возможностях.
#[derive(Debug, Clone)]
pub(crate) struct DeviceDetails {
    /// Имя драйвера (`VkPhysicalDeviceDriverProperties::driverName`).
    pub driver_name: String,
    /// Строка информации о драйвере.
    pub driver_info: String,
    /// Идентификатор драйвера.
    pub driver_id: vk::DriverId,
    /// Наносекунд на тик timestamp.
    pub timestamp_period_ns: f32,
    /// Управление округлением и денормалями f32 (`VkPhysicalDeviceFloatControlsProperties`).
    pub float32_controls: Float32Controls,
    /// Размер subgroup по умолчанию.
    pub subgroup_size: u32,
    /// Арифметика subgroup (`subgroupAdd` и др.) доступна в compute-шейдерах.
    pub subgroup_arithmetic_in_compute: bool,
    /// Максимальный размер 2D-картинки.
    pub max_image_dimension_2d: u32,
    /// Максимальный размер push-констант, байт (спецификация гарантирует ≥ 128).
    pub max_push_constants_size: u32,
    /// Максимальное число рабочих групп одного dispatch по осям X, Y, Z.
    pub max_compute_work_group_count: [u32; 3],
    /// Максимум нитей в одной рабочей группе (`maxComputeWorkGroupInvocations`, спецификация гарантирует ≥ 128).
    pub max_compute_work_group_invocations: u32,
    /// Максимальный размер рабочей группы по осям X, Y, Z (`maxComputeWorkGroupSize`).
    pub max_compute_work_group_size: [u32; 3],
    /// Наибольший диапазон storage-буфера в дескрипторе, байт (`maxStorageBufferRange`).
    pub max_storage_buffer_range: u32,
    /// Наибольший диапазон uniform-буфера в дескрипторе, байт (`maxUniformBufferRange`).
    pub max_uniform_buffer_range: u32,
    /// Типы и кучи памяти.
    pub memory_properties: vk::PhysicalDeviceMemoryProperties,
}

/// Физическое устройство вместе с отчётом для выбора.
#[derive(Debug, Clone)]
pub(crate) struct ProbedDevice {
    pub physical_device: vk::PhysicalDevice,
    pub candidate: DeviceCandidate,
    pub details: DeviceDetails,
}

/// Опрашивает все физические устройства instance.
pub(crate) fn probe_devices(instance: &Instance) -> Result<Vec<ProbedDevice>, GpuInitError> {
    // SAFETY: instance валиден.
    let devices = init_step(
        unsafe { instance.enumerate_physical_devices() },
        "vkEnumeratePhysicalDevices",
    )?;
    devices
        .into_iter()
        .map(|physical_device| probe_device(instance, physical_device))
        .collect()
}

fn probe_device(instance: &Instance, physical_device: vk::PhysicalDevice) -> Result<ProbedDevice, GpuInitError> {
    // SAFETY: physical_device получен из этого instance.
    let extension_properties = init_step(
        unsafe { instance.enumerate_device_extension_properties(physical_device) },
        "vkEnumerateDeviceExtensionProperties",
    )?;
    let extensions: Vec<String> = extension_properties
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let has_swapchain_maintenance1 = extensions
        .iter()
        .any(|name| name.as_str() == ext::swapchain_maintenance1::NAME.to_string_lossy());

    // Свойства: base + Vulkan 1.1 (subgroup) + 1.2 (driver).
    let mut properties_11 = vk::PhysicalDeviceVulkan11Properties::default();
    let mut properties_12 = vk::PhysicalDeviceVulkan12Properties::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default()
        .push_next(&mut properties_11)
        .push_next(&mut properties_12);
    // SAFETY: цепочка pNext состоит из живых локальных структур с корректными sType.
    unsafe { instance.get_physical_device_properties2(physical_device, &mut properties2) };
    let properties = properties2.properties;

    // Возможности: base + 1.1 + 1.2 + 1.3 (+ swapchain_maintenance1, только если расширение есть).
    let mut features_11 = vk::PhysicalDeviceVulkan11Features::default();
    let mut features_12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut features_13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut maintenance1_features = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
    let mut features2 = vk::PhysicalDeviceFeatures2::default()
        .push_next(&mut features_11)
        .push_next(&mut features_12)
        .push_next(&mut features_13);
    if has_swapchain_maintenance1 {
        features2 = features2.push_next(&mut maintenance1_features);
    }
    // SAFETY: то же — живые локальные структуры; maintenance1 в цепочке только при наличии расширения.
    unsafe { instance.get_physical_device_features2(physical_device, &mut features2) };
    let base_features = features2.features;

    // SAFETY: physical_device валиден.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    // SAFETY: physical_device валиден.
    let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical_device) };

    let candidate = DeviceCandidate {
        name: properties
            .device_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "<имя не UTF-8>".to_owned()),
        device_type: properties.device_type,
        api_version: properties.api_version,
        extensions,
        queue_families: families
            .iter()
            .map(|family| QueueFamilyReport {
                flags: family.queue_flags,
                queue_count: family.queue_count,
                timestamp_valid_bits: family.timestamp_valid_bits,
            })
            .collect(),
        features: FeatureSupport {
            dynamic_rendering: features_13.dynamic_rendering == vk::TRUE,
            synchronization2: features_13.synchronization2 == vk::TRUE,
            timeline_semaphore: features_12.timeline_semaphore == vk::TRUE,
            maintenance4: features_13.maintenance4 == vk::TRUE,
            storage_image_extended_formats: base_features.shader_storage_image_extended_formats == vk::TRUE,
            swapchain_maintenance1: maintenance1_features.swapchain_maintenance1 == vk::TRUE,
            shader_float16: features_12.shader_float16 == vk::TRUE,
            storage_buffer_16bit_access: features_11.storage_buffer16_bit_access == vk::TRUE,
            host_query_reset: features_12.host_query_reset == vk::TRUE,
        },
    };
    let details = DeviceDetails {
        driver_name: properties_12
            .driver_name_as_c_str()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        driver_info: properties_12
            .driver_info_as_c_str()
            .map(|info| info.to_string_lossy().into_owned())
            .unwrap_or_default(),
        driver_id: properties_12.driver_id,
        timestamp_period_ns: properties.limits.timestamp_period,
        float32_controls: Float32Controls {
            rounding_mode_rte: properties_12.shader_rounding_mode_rte_float32 == vk::TRUE,
            denorm_preserve: properties_12.shader_denorm_preserve_float32 == vk::TRUE,
            denorm_flush_to_zero: properties_12.shader_denorm_flush_to_zero_float32 == vk::TRUE,
        },
        subgroup_size: properties_11.subgroup_size,
        // Модуль с capability GroupNonUniformArithmetic устройство принимает только при бите ARITHMETIC,
        // а сама операция в compute — только если compute есть среди поддерживаемых стадий subgroup.
        subgroup_arithmetic_in_compute: properties_11
            .subgroup_supported_operations
            .contains(vk::SubgroupFeatureFlags::ARITHMETIC)
            && properties_11
                .subgroup_supported_stages
                .contains(vk::ShaderStageFlags::COMPUTE),
        max_image_dimension_2d: properties.limits.max_image_dimension2_d,
        max_push_constants_size: properties.limits.max_push_constants_size,
        max_compute_work_group_count: properties.limits.max_compute_work_group_count,
        max_compute_work_group_invocations: properties.limits.max_compute_work_group_invocations,
        max_compute_work_group_size: properties.limits.max_compute_work_group_size,
        max_storage_buffer_range: properties.limits.max_storage_buffer_range,
        max_uniform_buffer_range: properties.limits.max_uniform_buffer_range,
        memory_properties,
    };
    Ok(ProbedDevice {
        physical_device,
        candidate,
        details,
    })
}
