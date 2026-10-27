//! Конфигурация ISP видоискателя ([`ViewfinderIspConfig`]): serde, неизвестные поля — ошибка.
//!
//! Значения по умолчанию и их источник — у каждого поля. Проверка — [`ViewfinderIspConfig::validate`]
//! (зовётся из `ViewfinderIsp::new` до создания объектов Vulkan).

use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_model::statistics::AwbStatisticsSettings;

use super::awb_reduction::AwbReductionPolicy;
use super::error::ViewfinderError;

/// Предпочтение формата изображения видоискателя (зеркало `StorageFormatPolicy` для конфига).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewfinderStorageFormat {
    /// 10 бит UNORM (4 байта на пиксель), если устройство пишет его как storage image; иначе fp16.
    /// Шаг 1/1023 мельче шага 8-битного экрана — для видоискателя достаточно, трафик вдвое меньше fp16.
    #[default]
    PreferTenBitUnorm,
    /// Только fp16 (паритетные тесты, отладка).
    HalfFloatOnly,
}

impl ViewfinderStorageFormat {
    /// Политика выбора формата `gpu-vulkan`.
    pub fn policy(self) -> StorageFormatPolicy {
        match self {
            Self::PreferTenBitUnorm => StorageFormatPolicy::PreferTenBitUnorm,
            Self::HalfFloatOnly => StorageFormatPolicy::HalfFloatOnly,
        }
    }
}

/// Размер рабочей группы (нитей по X и Y; одна нить — один пиксель выхода).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkgroupSizeConfig {
    /// Нитей по X.
    pub x: u32,
    /// Нитей по Y.
    pub y: u32,
}

impl Default for WorkgroupSizeConfig {
    /// 16×8 = 128 нитей — минимум `maxComputeWorkGroupInvocations`, который Vulkan гарантирует на любом
    /// устройстве, и кратно волне Adreno (64) и RADV (64). Лучший размер подбирается замером на телефоне.
    fn default() -> Self {
        Self { x: 16, y: 8 }
    }
}

/// Верхняя граница нитей в группе, которую принимает КОНФИГ (проверка без GPU): `maxComputeWorkGroupInvocations`
/// у обоих наших GPU (Adreno 740 и RADV — 1024, vkjson). Настоящий лимит устройства (нитей и размер по
/// осям, `maxComputeWorkGroupSize`) проверяется в `ViewfinderIsp::new` через `LocalSize::validate_for_device`:
/// на устройстве с меньшим лимитом (спецификация гарантирует лишь 128) — чистая ошибка, а не UB.
pub const MAX_WORKGROUP_INVOCATIONS: u32 = 1024;

/// Верхняя граница слотов кадров: больше двух-трёх кадров в полёте видоискателю не нужно никогда.
pub const MAX_FRAME_SLOTS: usize = 8;

/// Конфигурация ISP видоискателя.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderIspConfig {
    /// Слотов кадров (uniform-параметры, статистика, метки времени, наборы дескрипторов — по слоту).
    /// По умолчанию 2 — столько кадров в полёте держит поток рендера.
    pub frame_slots: usize,
    /// Размер рабочей группы (specialization-константа шейдера).
    pub workgroup_size: WorkgroupSizeConfig,
    /// Сетка и порог статистики AWB (по умолчанию — `isp-model`: 32×24, порог 0,95).
    pub awb_statistics: AwbStatisticsSettings,
    /// Как сводить статистику AWB внутри рабочей группы (по умолчанию — subgroup, если устройство умеет;
    /// на телефоне схема через shared-окно стоила ~3,3 мс GPU на кадр 4:3, сессия 07).
    pub awb_reduction: AwbReductionPolicy,
    /// Формат изображения видоискателя.
    pub storage_format: ViewfinderStorageFormat,
    /// Сколько вариантов пайплайна (набор стадий × формат × источник) держать одновременно.
    /// По умолчанию 12: продукт + 8 A/B-вариантов «без одной стадии» + запас; при переполнении
    /// давно не использованный вариант уходит в отложенное уничтожение.
    pub pipeline_variant_capacity: usize,
}

impl Default for ViewfinderIspConfig {
    fn default() -> Self {
        Self {
            frame_slots: 2,
            workgroup_size: WorkgroupSizeConfig::default(),
            awb_statistics: AwbStatisticsSettings::default(),
            awb_reduction: AwbReductionPolicy::default(),
            storage_format: ViewfinderStorageFormat::default(),
            pipeline_variant_capacity: 12,
        }
    }
}

impl ViewfinderIspConfig {
    /// Проверяет конфиг; первая найденная проблема — `InvalidConfig` с объяснением.
    pub fn validate(&self) -> Result<(), ViewfinderError> {
        let invalid = |what: String| Err(ViewfinderError::InvalidConfig { what });
        if self.frame_slots == 0 || self.frame_slots > MAX_FRAME_SLOTS {
            return invalid(format!("frame_slots {} вне 1..={MAX_FRAME_SLOTS}", self.frame_slots));
        }
        let WorkgroupSizeConfig { x, y } = self.workgroup_size;
        let invocations = u64::from(x) * u64::from(y);
        if x == 0 || y == 0 || invocations > u64::from(MAX_WORKGROUP_INVOCATIONS) {
            return invalid(format!(
                "рабочая группа {x}x{y}: нужно x, y > 0 и x·y ≤ {MAX_WORKGROUP_INVOCATIONS}"
            ));
        }
        if self.pipeline_variant_capacity == 0 {
            return invalid("pipeline_variant_capacity должен быть ≥ 1".to_owned());
        }
        let settings = self.awb_statistics;
        if settings.grid.columns == 0 || settings.grid.rows == 0 {
            return invalid(format!("пустая сетка статистики AWB {:?}", settings.grid));
        }
        let threshold = settings.saturation_threshold;
        if !(threshold.is_finite() && threshold > 0.0 && threshold <= 1.0) {
            return invalid(format!("порог пересвета AWB {threshold} вне (0, 1]"));
        }
        Ok(())
    }

    /// Размер рабочей группы для `gpu-vulkan` (после [`Self::validate`] ошибки нет).
    pub fn local_size(&self) -> Result<LocalSize, ViewfinderError> {
        Ok(LocalSize::new(self.workgroup_size.x, self.workgroup_size.y)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid_and_round_trips_through_json() {
        let config = ViewfinderIspConfig::default();
        assert_eq!(config.validate(), Ok(()));
        let text = serde_json::to_string(&config).unwrap();
        let parsed: ViewfinderIspConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, config);
        // Частичный JSON берёт остальное из значений по умолчанию.
        let partial: ViewfinderIspConfig = serde_json::from_str(r#"{"frame_slots": 3}"#).unwrap();
        assert_eq!(partial.frame_slots, 3);
        assert_eq!(partial.workgroup_size, WorkgroupSizeConfig { x: 16, y: 8 });
    }

    #[test]
    fn unknown_fields_and_invalid_values_are_rejected() {
        assert!(serde_json::from_str::<ViewfinderIspConfig>(r#"{"frame_slot": 2}"#).is_err());
        let base = ViewfinderIspConfig::default();
        let broken = [
            ViewfinderIspConfig { frame_slots: 0, ..base },
            ViewfinderIspConfig {
                frame_slots: MAX_FRAME_SLOTS + 1,
                ..base
            },
            ViewfinderIspConfig {
                workgroup_size: WorkgroupSizeConfig { x: 0, y: 8 },
                ..base
            },
            ViewfinderIspConfig {
                workgroup_size: WorkgroupSizeConfig { x: 64, y: 32 },
                ..base
            },
            ViewfinderIspConfig {
                pipeline_variant_capacity: 0,
                ..base
            },
            ViewfinderIspConfig {
                awb_statistics: AwbStatisticsSettings {
                    saturation_threshold: 1.5,
                    ..base.awb_statistics
                },
                ..base
            },
        ];
        for config in broken {
            assert!(
                matches!(config.validate(), Err(ViewfinderError::InvalidConfig { .. })),
                "{config:?}"
            );
        }
    }

    #[test]
    fn storage_format_maps_to_gpu_policy() {
        assert_eq!(
            ViewfinderStorageFormat::PreferTenBitUnorm.policy(),
            StorageFormatPolicy::PreferTenBitUnorm
        );
        assert_eq!(
            ViewfinderStorageFormat::HalfFloatOnly.policy(),
            StorageFormatPolicy::HalfFloatOnly
        );
    }
}
