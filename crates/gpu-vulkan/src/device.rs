//! Профили устройства и ЧИСТАЯ логика выбора физического устройства.
//!
//! Выбор написан как функция над отчётами об устройствах ([`DeviceCandidate`]), поэтому
//! тестируется без драйвера: тест подставляет любые списки расширений/фич/очередей и
//! проверяет причины отказа. Чтение отчётов из настоящего Vulkan — в [`probe`].

use std::ffi::CStr;

use ash::{ext, google, khr, vk};

use crate::error::{DeviceRejection, DeviceRejectionReason, GpuInitError};

pub(crate) mod probe;

/// Профиль устройства: какой набор расширений контекст обязан и может включить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceProfile {
    /// Телефон: показ через Android surface, импорт AHardwareBuffer, sync_fd.
    AndroidDisplay,
    /// Без показа (ПК, вычисления и offscreen).
    Headless,
    /// ПК-стенд swapchain: `Headless` + `VK_EXT_headless_surface` и swapchain.
    HeadlessPresentation,
}

/// Списки расширений: обязательные (нет — ошибка) и необязательные (включаются, если есть).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionRequirements {
    /// Без них профиль неработоспособен.
    pub required: Vec<&'static CStr>,
    /// Включаются, если доступны.
    pub optional: Vec<&'static CStr>,
}

impl DeviceProfile {
    /// Расширения instance, нужные профилю.
    pub fn instance_extensions(self) -> ExtensionRequirements {
        match self {
            DeviceProfile::AndroidDisplay => ExtensionRequirements {
                required: vec![khr::surface::NAME, khr::android_surface::NAME],
                // `swapchain_colorspace` — необязательно (сессия 07): без него доступен только sRGB,
                // с ним swapchain может выбрать Display P3 (см. `swapchain::policy`).
                optional: vec![
                    khr::get_surface_capabilities2::NAME,
                    ext::surface_maintenance1::NAME,
                    ext::swapchain_colorspace::NAME,
                ],
            },
            DeviceProfile::Headless => ExtensionRequirements::default(),
            DeviceProfile::HeadlessPresentation => ExtensionRequirements {
                required: vec![khr::surface::NAME, ext::headless_surface::NAME],
                // Как на Android: нужны, чтобы стенд проверял путь present-fence'ов (swapchain_maintenance1)
                // и выбор цветового пространства (swapchain_colorspace).
                optional: vec![
                    khr::get_surface_capabilities2::NAME,
                    ext::surface_maintenance1::NAME,
                    ext::swapchain_colorspace::NAME,
                ],
            },
        }
    }

    /// Расширения устройства, нужные профилю.
    pub fn device_extensions(self) -> ExtensionRequirements {
        match self {
            DeviceProfile::AndroidDisplay => ExtensionRequirements {
                required: vec![
                    khr::swapchain::NAME,
                    ash::android::external_memory_android_hardware_buffer::NAME,
                    vk::EXT_QUEUE_FAMILY_FOREIGN_NAME,
                    khr::external_semaphore_fd::NAME,
                    khr::external_fence_fd::NAME,
                    google::display_timing::NAME,
                ],
                optional: vec![ext::swapchain_maintenance1::NAME],
            },
            DeviceProfile::Headless => ExtensionRequirements {
                required: vec![khr::external_semaphore_fd::NAME],
                optional: vec![khr::external_fence_fd::NAME],
            },
            DeviceProfile::HeadlessPresentation => ExtensionRequirements {
                required: vec![khr::external_semaphore_fd::NAME, khr::swapchain::NAME],
                optional: vec![khr::external_fence_fd::NAME, ext::swapchain_maintenance1::NAME],
            },
        }
    }
}

/// Какие возможности устройства реально поддержаны (из `vkGetPhysicalDeviceFeatures2`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FeatureSupport {
    /// Vulkan 1.3 `dynamicRendering` (обязательна).
    pub dynamic_rendering: bool,
    /// Vulkan 1.3 `synchronization2` (обязательна).
    pub synchronization2: bool,
    /// Vulkan 1.2 `timelineSemaphore` (обязательна).
    pub timeline_semaphore: bool,
    /// Vulkan 1.3 `maintenance4` (обязательна). glslc с `--target-env=vulkan1.3` пишет в каждый
    /// compute-шейдер SPIR-V 1.6 `OpExecutionModeId LocalSizeId`, а он разрешён только с этой
    /// фичей (VUID-RuntimeSpirv-LocalSizeId-06434). В ядре 1.3 её поддержка обязательна для
    /// любого драйвера (спецификация, раздел «Feature Requirements»).
    pub maintenance4: bool,
    /// `shaderStorageImageExtendedFormats` (включается, если есть; нужна для `r16ui`).
    pub storage_image_extended_formats: bool,
    /// `VK_EXT_swapchain_maintenance1::swapchainMaintenance1`.
    pub swapchain_maintenance1: bool,
    /// Vulkan 1.2 `shaderFloat16` (необязательна, сессия 07): арифметика fp16 в шейдерах ISP.
    pub shader_float16: bool,
    /// Vulkan 1.1 `storageBuffer16BitAccess` (необязательна, сессия 07): 16-битные значения в storage-буферах.
    pub storage_buffer_16bit_access: bool,
    /// Vulkan 1.2 `hostQueryReset` (необязательна, сессия 07): сброс запросов меток времени с CPU
    /// (`vkResetQueryPool`). Есть у RADV и Adreno 740; без неё метки сбрасываются командой в буфере.
    pub host_query_reset: bool,
}

/// Семейство очередей физического устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueFamilyReport {
    /// Флаги возможностей.
    pub flags: vk::QueueFlags,
    /// Сколько очередей в семействе.
    pub queue_count: u32,
    /// Число значимых бит timestamp (0 — таймстемпы не поддержаны).
    pub timestamp_valid_bits: u32,
}

/// Отчёт об одном физическом устройстве — вход чистого выбора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCandidate {
    /// Имя из драйвера.
    pub name: String,
    /// Тип устройства (дискретное/встроенное/CPU).
    pub device_type: vk::PhysicalDeviceType,
    /// Версия API (`vk::make_api_version`).
    pub api_version: u32,
    /// Имена поддерживаемых расширений устройства.
    pub extensions: Vec<String>,
    /// Семейства очередей.
    pub queue_families: Vec<QueueFamilyReport>,
    /// Поддержанные возможности.
    pub features: FeatureSupport,
}

/// Итог выбора: какое устройство, какая очередь и какие расширения включать.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSelection {
    /// Индекс устройства в переданном списке.
    pub device_index: usize,
    /// Семейство очередей GRAPHICS|COMPUTE.
    pub queue_family_index: u32,
    /// Расширения для `vkCreateDevice` (обязательные + найденные необязательные).
    pub enabled_extensions: Vec<&'static CStr>,
}

/// Минимальная версия API (решение сессии 03: целевой Vulkan 1.3).
pub const REQUIRED_API_VERSION: u32 = vk::API_VERSION_1_3;

/// Проверяет одно устройство. `Err` — причина отказа (первая найденная).
///
/// `enabled_instance_extensions` нужен, чтобы не включать необязательное расширение устройства,
/// у которого нет своей части на уровне instance (`swapchain_maintenance1` требует
/// `VK_EXT_surface_maintenance1` в instance).
pub fn evaluate_candidate(
    candidate: &DeviceCandidate,
    requirements: &ExtensionRequirements,
    enabled_instance_extensions: &[&CStr],
) -> Result<DeviceSelection, DeviceRejectionReason> {
    if candidate.api_version < REQUIRED_API_VERSION {
        return Err(DeviceRejectionReason::ApiVersionTooLow {
            found: format!(
                "{}.{}",
                vk::api_version_major(candidate.api_version),
                vk::api_version_minor(candidate.api_version)
            ),
        });
    }

    let has_extension = |name: &CStr| {
        let wanted = name.to_string_lossy();
        candidate.extensions.iter().any(|available| *available == wanted)
    };

    let mut enabled_extensions = Vec::new();
    for required in &requirements.required {
        if !has_extension(required) {
            return Err(DeviceRejectionReason::MissingDeviceExtension {
                name: required.to_string_lossy().into_owned(),
            });
        }
        enabled_extensions.push(*required);
    }

    let features = candidate.features;
    for (supported, name) in [
        (features.dynamic_rendering, "dynamicRendering"),
        (features.synchronization2, "synchronization2"),
        (features.timeline_semaphore, "timelineSemaphore"),
        (features.maintenance4, "maintenance4"),
    ] {
        if !supported {
            return Err(DeviceRejectionReason::MissingFeature { name: name.to_owned() });
        }
    }

    let queue_family_index = candidate
        .queue_families
        .iter()
        .position(|family| {
            family.queue_count > 0
                && family
                    .flags
                    .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
        })
        .ok_or(DeviceRejectionReason::NoGraphicsComputeQueue)?;

    for optional in &requirements.optional {
        if !has_extension(optional) {
            continue;
        }
        if *optional == ext::swapchain_maintenance1::NAME
            && !(features.swapchain_maintenance1
                && instance_has(enabled_instance_extensions, ext::surface_maintenance1::NAME))
        {
            continue;
        }
        enabled_extensions.push(*optional);
    }

    Ok(DeviceSelection {
        device_index: 0,
        queue_family_index: u32::try_from(queue_family_index).unwrap_or(0),
        enabled_extensions,
    })
}

fn instance_has(enabled: &[&CStr], name: &CStr) -> bool {
    enabled.contains(&name)
}

/// Предпочтение типов: дискретная карта лучше встроенной, встроенная лучше виртуальной и CPU.
fn device_type_rank(device_type: vk::PhysicalDeviceType) -> u8 {
    match device_type {
        vk::PhysicalDeviceType::DISCRETE_GPU => 0,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
        vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
        vk::PhysicalDeviceType::CPU => 3,
        _ => 4,
    }
}

/// Выбирает лучшее подходящее устройство или объясняет, почему подходящего нет.
///
/// Правило ошибки: если устройства есть и ВСЕ отвергнуты по одной и той же причине вида
/// «нет расширения X» / «нет возможности Y», возвращается точная ошибка
/// `MissingDeviceExtension` / `MissingFeature` (на телефоне устройство одно, и вызывающему
/// нужно конкретное имя). Иначе — `NoSuitableDevice` с причиной по каждому устройству.
pub fn select_device(
    candidates: &[DeviceCandidate],
    requirements: &ExtensionRequirements,
    enabled_instance_extensions: &[&CStr],
) -> Result<DeviceSelection, GpuInitError> {
    let mut best: Option<(u8, DeviceSelection)> = None;
    let mut rejected = Vec::new();

    for (device_index, candidate) in candidates.iter().enumerate() {
        match evaluate_candidate(candidate, requirements, enabled_instance_extensions) {
            Ok(mut selection) => {
                selection.device_index = device_index;
                let rank = device_type_rank(candidate.device_type);
                // `<` (а не `<=`): при равенстве рангов остаётся первое устройство в порядке драйвера.
                if best.as_ref().is_none_or(|(best_rank, _)| rank < *best_rank) {
                    best = Some((rank, selection));
                }
            }
            Err(reason) => rejected.push(DeviceRejection {
                device_name: candidate.name.clone(),
                reason,
            }),
        }
    }

    if let Some((_, selection)) = best {
        return Ok(selection);
    }
    Err(explain_rejections(rejected))
}

/// Сворачивает список отказов в точную ошибку (см. правило в [`select_device`]).
fn explain_rejections(rejected: Vec<DeviceRejection>) -> GpuInitError {
    if let Some(first) = rejected.first() {
        let all_same = rejected.iter().all(|rejection| rejection.reason == first.reason);
        if all_same {
            match &first.reason {
                DeviceRejectionReason::MissingDeviceExtension { name } => {
                    return GpuInitError::MissingDeviceExtension { name: name.clone() };
                }
                DeviceRejectionReason::MissingFeature { name } => {
                    return GpuInitError::MissingFeature { name: name.clone() };
                }
                DeviceRejectionReason::ApiVersionTooLow { .. } | DeviceRejectionReason::NoGraphicsComputeQueue => {}
            }
        }
    }
    GpuInitError::NoSuitableDevice { rejected }
}

#[cfg(test)]
mod tests;
