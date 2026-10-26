//! Секция `gpu` конфига нативной части: как включается validation layer Vulkan.
//!
//! Validation layer — проверка каждого вызова Vulkan (медленная, но ловит ошибки владения и
//! синхронизации). Включена она только там, где есть сам слой: в debug-APK (его кладёт туда
//! `scripts/fetch-validation-layer.sh`, решение 34). По умолчанию режим `auto` берёт решение
//! из типа сборки, чтобы Kotlin ничего не присылал.

use gpu_vulkan::validation::{KnownValidationDeviation, KnownValidationDeviations, ValidationMode};
use serde::{Deserialize, Serialize};

use crate::runtime_config::BuildType;

/// Что запросили для validation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSetting {
    /// Решает тип сборки: debug — полная проверка, release — выключено.
    Auto,
    /// Всегда включить (нет слоя в APK — GPU не поднимется и это будет видно в отчёте).
    Enabled,
    /// Всегда выключить.
    Disabled,
}

/// Одно известное отступление от правил Vulkan (решение 35): validation-сообщение с этим VUID
/// не считается ошибкой, а учитывается отдельным счётчиком вместе с причиной.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownValidationDeviationConfig {
    /// Точное имя сообщения слоя (`pMessageIdName`), сравнение целиком, не по префиксу.
    pub vuid: String,
    /// Почему отступление осознанно допустимо.
    pub reason: String,
}

/// Настройки GPU (секция `gpu`). Неизвестные поля — ошибка, отсутствующие — значения по умолчанию.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GpuConfig {
    /// `auto` по умолчанию: debug-сборка проверяется слоем, release работает без него.
    pub validation: ValidationSetting,
    /// Известные отступления от правил Vulkan (решение 35). Присланный список ЗАМЕНЯЕТ
    /// умолчание целиком: пустой список означает «любая ошибка validation — провал».
    pub known_validation_deviations: Vec<KnownValidationDeviationConfig>,
}

impl GpuConfig {
    /// VUID импорта RAW16: у формата нет эквивалента Vulkan, `externalFormat` драйвер отдаёт 0.
    pub const VUID_RAW16_FORMAT_MISMATCH: &'static str = "VUID-VkMemoryAllocateInfo-pNext-02387";
    /// VUID импорта RAW10 в буфер: спецификация требует BLOB + `GPU_DATA_BUFFER`.
    pub const VUID_RAW10_BUFFER_USAGE: &'static str = "VUID-VkMemoryAllocateInfo-pNext-02384";

    /// Два отступления по умолчанию (решение 35, факты 00 C4): единственный путь импорта без копии.
    fn default_known_validation_deviations() -> Vec<KnownValidationDeviationConfig> {
        vec![
            KnownValidationDeviationConfig {
                vuid: Self::VUID_RAW16_FORMAT_MISMATCH.to_owned(),
                reason: "Решение 35, 00 C4: у RAW16 нет эквивалентного формата Vulkan, а externalFormat драйвер \
                         отдаёт 0; импорт как R16_UINT без копии — единственный путь, Adreno принимает его, данные \
                         совпадают бит-в-бит"
                    .to_owned(),
            },
            KnownValidationDeviationConfig {
                vuid: Self::VUID_RAW10_BUFFER_USAGE.to_owned(),
                reason: "Решение 35, 00 C4: импорт RAW10 в буфер формально требует BLOB + GPU_DATA_BUFFER, а с таким \
                         usage камера кадров не отдаёт; Adreno принимает импорт, данные совпадают бит-в-бит"
                    .to_owned(),
            },
        ]
    }

    /// Список отступлений в виде, который понимает `gpu-vulkan`.
    pub fn known_deviations(&self) -> KnownValidationDeviations {
        KnownValidationDeviations::new(
            self.known_validation_deviations
                .iter()
                .map(|entry| KnownValidationDeviation {
                    vuid: entry.vuid.clone(),
                    reason: entry.reason.clone(),
                })
                .collect(),
        )
    }
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            validation: ValidationSetting::Auto,
            known_validation_deviations: Self::default_known_validation_deviations(),
        }
    }
}

impl GpuConfig {
    /// Итоговый режим validation для типа сборки.
    pub fn validation_mode(&self, build_type: BuildType) -> ValidationMode {
        match (self.validation, build_type) {
            (ValidationSetting::Enabled, _) | (ValidationSetting::Auto, BuildType::Debug) => {
                ValidationMode::full_with_known_deviations(self.known_deviations())
            }
            (ValidationSetting::Disabled, _) | (ValidationSetting::Auto, BuildType::Release) => {
                ValidationMode::Disabled
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mode(build_type: BuildType) -> ValidationMode {
        GpuConfig::default().validation_mode(build_type)
    }

    #[test]
    fn auto_follows_build_type() {
        assert_eq!(default_mode(BuildType::Release), ValidationMode::Disabled);
        let debug = default_mode(BuildType::Debug);
        assert!(debug.is_enabled() && debug.wants_synchronization());
    }

    #[test]
    fn explicit_setting_overrides_build_type() {
        let enabled = GpuConfig {
            validation: ValidationSetting::Enabled,
            ..GpuConfig::default()
        };
        let disabled = GpuConfig {
            validation: ValidationSetting::Disabled,
            ..GpuConfig::default()
        };
        assert!(enabled.validation_mode(BuildType::Release).is_enabled());
        assert_eq!(disabled.validation_mode(BuildType::Debug), ValidationMode::Disabled);
    }

    #[test]
    fn defaults_are_exactly_the_two_decision_35_deviations_with_reasons() {
        let config = GpuConfig::default();
        let vuids: Vec<&str> = config
            .known_validation_deviations
            .iter()
            .map(|entry| entry.vuid.as_str())
            .collect();
        assert_eq!(
            vuids,
            [
                "VUID-VkMemoryAllocateInfo-pNext-02387",
                "VUID-VkMemoryAllocateInfo-pNext-02384"
            ]
        );
        assert!(
            config
                .known_validation_deviations
                .iter()
                .all(|entry| entry.reason.contains("Решение 35"))
        );
    }

    #[test]
    fn debug_mode_carries_the_configured_deviations_into_gpu_vulkan() {
        let mode = default_mode(BuildType::Debug);
        let carried: Vec<String> = mode
            .known_deviations()
            .entries()
            .iter()
            .map(|entry| entry.vuid.clone())
            .collect();
        assert_eq!(
            carried,
            [
                GpuConfig::VUID_RAW16_FORMAT_MISMATCH,
                GpuConfig::VUID_RAW10_BUFFER_USAGE
            ]
        );
        // Выключенная validation отступлений не несёт.
        assert!(default_mode(BuildType::Release).known_deviations().entries().is_empty());
    }

    #[test]
    fn deviations_can_be_overridden_and_emptied_from_json() {
        let overridden: GpuConfig = serde_json::from_str(
            r#"{"known_validation_deviations": [{"vuid": "VUID-Test-00001", "reason": "причина"}]}"#,
        )
        .expect("корректный конфиг");
        assert_eq!(
            overridden.known_validation_deviations,
            [KnownValidationDeviationConfig {
                vuid: "VUID-Test-00001".to_owned(),
                reason: "причина".to_owned(),
            }]
        );
        assert_eq!(overridden.validation, ValidationSetting::Auto, "остальное — умолчания");

        let emptied: GpuConfig = serde_json::from_str(r#"{"known_validation_deviations": []}"#).expect("корректный");
        assert!(emptied.known_validation_deviations.is_empty());
        assert!(
            emptied
                .validation_mode(BuildType::Debug)
                .known_deviations()
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn unknown_fields_are_rejected_at_both_levels() {
        assert!(serde_json::from_str::<GpuConfig>(r#"{"known_validation_deviation": []}"#).is_err());
        assert!(
            serde_json::from_str::<GpuConfig>(
                r#"{"known_validation_deviations": [{"vuid": "a", "reason": "b", "x": 1}]}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<GpuConfig>(r#"{"known_validation_deviations": [{"vuid": "a"}]}"#).is_err(),
            "причина обязательна"
        );
    }
}
