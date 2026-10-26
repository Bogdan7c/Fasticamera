//! Создание `VkInstance`: план расширений (чистая функция) и сам вызов Vulkan.
//!
//! План отделён от создания, чтобы логику «что включать и когда ошибка» проверять без драйвера.

use std::ffi::{CStr, CString, c_void};

use ash::{Entry, Instance, ext, vk};

use crate::device::ExtensionRequirements;
use crate::error::{GpuInitError, init_step};
use crate::validation::{VALIDATION_LAYER_NAME, ValidationHook, ValidationMode, messenger_create_info};

/// Каким способом включается sync-валидация слоя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncValidationBackend {
    /// Sync-валидацию не просили.
    NotRequested,
    /// Через `VK_EXT_layer_settings` (`validate_sync = true`) — основной способ.
    LayerSettings,
    /// Запасной способ: `VK_EXT_validation_features` + `SYNCHRONIZATION_VALIDATION`.
    ValidationFeatures,
    /// Просили, но слой не предлагает ни один из способов. Контекст работает без sync-валидации,
    /// факт виден в [`crate::context::GpuCapabilities::sync_validation`].
    Unavailable,
}

/// Итог планирования instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstancePlan {
    /// Расширения instance для `vkCreateInstance`.
    pub extensions: Vec<&'static CStr>,
    /// Способ включения sync-валидации.
    pub sync_validation: SyncValidationBackend,
}

fn contains(available: &[String], name: &CStr) -> bool {
    let wanted = name.to_string_lossy();
    available.iter().any(|candidate| *candidate == wanted)
}

/// Решает, какие расширения instance включить.
///
/// `available_extensions` — расширения загрузчика И слоя validation (если он есть).
/// `validation_layer_present` — найден ли слой в списке слоёв системы.
pub fn plan_instance(
    available_extensions: &[String],
    validation_layer_present: bool,
    requirements: &ExtensionRequirements,
    validation: &ValidationMode,
) -> Result<InstancePlan, GpuInitError> {
    if validation.is_enabled() && !validation_layer_present {
        return Err(GpuInitError::ValidationLayerUnavailable);
    }

    let mut extensions: Vec<&'static CStr> = Vec::new();
    for required in &requirements.required {
        if !contains(available_extensions, required) {
            return Err(GpuInitError::MissingInstanceExtension {
                name: required.to_string_lossy().into_owned(),
            });
        }
        extensions.push(*required);
    }
    for optional in &requirements.optional {
        if contains(available_extensions, optional) {
            extensions.push(*optional);
        }
    }

    let mut sync_validation = SyncValidationBackend::NotRequested;
    if validation.is_enabled() {
        // Без debug_utils сообщения слоя некуда доставить — это ошибка, а не тихое «ничего не видно».
        if !contains(available_extensions, ext::debug_utils::NAME) {
            return Err(GpuInitError::MissingInstanceExtension {
                name: ext::debug_utils::NAME.to_string_lossy().into_owned(),
            });
        }
        extensions.push(ext::debug_utils::NAME);

        if validation.wants_synchronization() {
            sync_validation = if contains(available_extensions, ext::layer_settings::NAME) {
                extensions.push(ext::layer_settings::NAME);
                SyncValidationBackend::LayerSettings
            } else if contains(available_extensions, ext::validation_features::NAME) {
                extensions.push(ext::validation_features::NAME);
                SyncValidationBackend::ValidationFeatures
            } else {
                SyncValidationBackend::Unavailable
            };
        }
    }
    extensions.dedup();
    Ok(InstancePlan {
        extensions,
        sync_validation,
    })
}

/// Читает расширения загрузчика и (если есть) слоя validation, а также наличие слоя.
pub(crate) fn query_instance_support(entry: &Entry) -> Result<(Vec<String>, bool), GpuInitError> {
    // SAFETY: entry загружен; None — расширения самого загрузчика.
    let global = init_step(
        unsafe { entry.enumerate_instance_extension_properties(None) },
        "vkEnumerateInstanceExtensionProperties",
    )?;
    // SAFETY: entry загружен.
    let layers = init_step(
        unsafe { entry.enumerate_instance_layer_properties() },
        "vkEnumerateInstanceLayerProperties",
    )?;
    let layer_present = layers.iter().any(|layer| {
        layer
            .layer_name_as_c_str()
            .is_ok_and(|name| name == VALIDATION_LAYER_NAME)
    });

    let mut names: Vec<String> = global
        .iter()
        .filter_map(|property| property.extension_name_as_c_str().ok())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if layer_present {
        // SAFETY: имя слоя — валидная C-строка.
        let from_layer = init_step(
            unsafe { entry.enumerate_instance_extension_properties(Some(VALIDATION_LAYER_NAME)) },
            "vkEnumerateInstanceExtensionProperties(layer)",
        )?;
        names.extend(
            from_layer
                .iter()
                .filter_map(|property| property.extension_name_as_c_str().ok())
                .map(|name| name.to_string_lossy().into_owned()),
        );
    }
    Ok((names, layer_present))
}

/// Имя приложения для `VkApplicationInfo`: нулевые байты вырезаются (C-строка).
fn application_name(label: &str) -> CString {
    let cleaned: Vec<u8> = label.bytes().filter(|byte| *byte != 0).collect();
    // Нулевых байт после фильтра нет, поэтому ошибка невозможна; пустое имя — безопасный запасной вариант.
    CString::new(cleaned).unwrap_or_default()
}

/// Создаёт instance по плану. Слой и messenger-на-время-создания подключаются только при validation.
pub(crate) fn create_instance(
    entry: &Entry,
    plan: &InstancePlan,
    validation: &ValidationMode,
    hook: &ValidationHook,
    app_label: &str,
) -> Result<Instance, GpuInitError> {
    let name = application_name(app_label);
    let application_info = vk::ApplicationInfo::default()
        .application_name(&name)
        .engine_name(c"fasticamera-gpu")
        .api_version(vk::API_VERSION_1_3);

    let extension_pointers: Vec<*const std::ffi::c_char> =
        plan.extensions.iter().map(|extension| extension.as_ptr()).collect();
    let layer_pointers = [VALIDATION_LAYER_NAME.as_ptr()];

    let mut create_info = vk::InstanceCreateInfo::default()
        .application_info(&application_info)
        .enabled_extension_names(&extension_pointers);
    if validation.is_enabled() {
        create_info = create_info.enabled_layer_names(&layer_pointers);
    }

    // Структуры цепочки pNext должны жить до вызова vkCreateInstance — объявляем их здесь.
    let mut creation_messenger = messenger_create_info(hook.user_data());
    let validate_sync_value = [vk::TRUE];
    let layer_settings = [vk::LayerSettingEXT {
        p_layer_name: VALIDATION_LAYER_NAME.as_ptr(),
        p_setting_name: c"validate_sync".as_ptr(),
        ty: vk::LayerSettingTypeEXT::BOOL32,
        value_count: 1,
        p_values: validate_sync_value.as_ptr().cast::<c_void>(),
        _marker: std::marker::PhantomData,
    }];
    let mut layer_settings_info = vk::LayerSettingsCreateInfoEXT::default().settings(&layer_settings);
    let enabled_validation_features = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
    let mut validation_features_info =
        vk::ValidationFeaturesEXT::default().enabled_validation_features(&enabled_validation_features);

    if validation.is_enabled() {
        create_info = create_info.push_next(&mut creation_messenger);
        match plan.sync_validation {
            SyncValidationBackend::LayerSettings => create_info = create_info.push_next(&mut layer_settings_info),
            SyncValidationBackend::ValidationFeatures => {
                create_info = create_info.push_next(&mut validation_features_info);
            }
            SyncValidationBackend::NotRequested | SyncValidationBackend::Unavailable => {}
        }
    }

    // SAFETY: create_info и всё, на что она ссылается (имена, цепочка pNext), живёт до конца вызова;
    // аллокатор по умолчанию.
    init_step(unsafe { entry.create_instance(&create_info, None) }, "vkCreateInstance")
}

#[cfg(test)]
mod tests {
    use ash::khr;

    use super::*;
    use crate::device::DeviceProfile;
    use crate::validation::SyncValidation;

    fn names(list: &[&CStr]) -> Vec<String> {
        list.iter().map(|name| name.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn missing_required_instance_extension_is_named() {
        let requirements = DeviceProfile::AndroidDisplay.instance_extensions();
        let available = names(&[khr::surface::NAME]);
        let error = plan_instance(&available, false, &requirements, &ValidationMode::Disabled).unwrap_err();
        assert!(
            matches!(&error, GpuInitError::MissingInstanceExtension { name } if name == "VK_KHR_android_surface"),
            "{error:?}"
        );
    }

    #[test]
    fn validation_without_layer_is_a_dedicated_error() {
        let error = plan_instance(&[], false, &ExtensionRequirements::default(), &ValidationMode::FULL).unwrap_err();
        assert!(matches!(error, GpuInitError::ValidationLayerUnavailable), "{error:?}");
    }

    #[test]
    fn full_validation_prefers_layer_settings_and_adds_debug_utils() {
        let available = names(&[
            ext::debug_utils::NAME,
            ext::layer_settings::NAME,
            ext::validation_features::NAME,
        ]);
        let plan = plan_instance(
            &available,
            true,
            &ExtensionRequirements::default(),
            &ValidationMode::FULL,
        )
        .unwrap();
        assert_eq!(plan.sync_validation, SyncValidationBackend::LayerSettings);
        assert!(plan.extensions.contains(&ext::debug_utils::NAME));
        assert!(plan.extensions.contains(&ext::layer_settings::NAME));
        assert!(!plan.extensions.contains(&ext::validation_features::NAME));
    }

    #[test]
    fn sync_validation_falls_back_to_validation_features_then_to_unavailable() {
        let with_features = names(&[ext::debug_utils::NAME, ext::validation_features::NAME]);
        let plan = plan_instance(
            &with_features,
            true,
            &ExtensionRequirements::default(),
            &ValidationMode::FULL,
        )
        .unwrap();
        assert_eq!(plan.sync_validation, SyncValidationBackend::ValidationFeatures);

        let bare = names(&[ext::debug_utils::NAME]);
        let plan = plan_instance(&bare, true, &ExtensionRequirements::default(), &ValidationMode::FULL).unwrap();
        assert_eq!(plan.sync_validation, SyncValidationBackend::Unavailable);
    }

    #[test]
    fn validation_without_debug_utils_is_an_error_not_silence() {
        let error = plan_instance(&[], true, &ExtensionRequirements::default(), &ValidationMode::FULL).unwrap_err();
        assert!(
            matches!(&error, GpuInitError::MissingInstanceExtension { name } if name == "VK_EXT_debug_utils"),
            "{error:?}"
        );
    }

    #[test]
    fn disabled_validation_adds_no_debug_extensions_and_basic_mode_skips_sync() {
        let available = names(&[ext::debug_utils::NAME, ext::layer_settings::NAME]);
        let plan = plan_instance(
            &available,
            true,
            &ExtensionRequirements::default(),
            &ValidationMode::Disabled,
        )
        .unwrap();
        assert!(plan.extensions.is_empty());
        assert_eq!(plan.sync_validation, SyncValidationBackend::NotRequested);

        let basic = ValidationMode::Enabled {
            synchronization: SyncValidation::Disabled,
            known_deviations: crate::validation::KnownValidationDeviations::none(),
        };
        let plan = plan_instance(&available, true, &ExtensionRequirements::default(), &basic).unwrap();
        assert_eq!(plan.extensions, vec![ext::debug_utils::NAME]);
        assert_eq!(plan.sync_validation, SyncValidationBackend::NotRequested);
    }

    #[test]
    fn optional_instance_extensions_are_taken_only_when_available() {
        let requirements = DeviceProfile::AndroidDisplay.instance_extensions();
        let available = names(&[
            khr::surface::NAME,
            khr::android_surface::NAME,
            ext::surface_maintenance1::NAME,
        ]);
        let plan = plan_instance(&available, false, &requirements, &ValidationMode::Disabled).unwrap();
        assert!(plan.extensions.contains(&ext::surface_maintenance1::NAME));
        assert!(!plan.extensions.contains(&khr::get_surface_capabilities2::NAME));
    }

    #[test]
    fn application_name_strips_nul_bytes() {
        assert_eq!(application_name("ab\0c").to_str().unwrap(), "abc");
    }
}
