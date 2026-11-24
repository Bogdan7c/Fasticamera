//! Конфигурация ISP видоискателя ([`ViewfinderIspConfig`]): serde, неизвестные поля — ошибка.
//!
//! Значения по умолчанию и их источник — у каждого поля. Проверка — [`ViewfinderIspConfig::validate`]
//! (зовётся из `ViewfinderIsp::new` до создания объектов Vulkan).

use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_model::statistics::{AwbStatisticsSettings, MAX_SUPPORTED_MOSAIC};

use super::awb_reduction::AwbReductionPolicy;
use super::error::ViewfinderError;
use crate::lens_shading_map::LensShadingMapLimits;
use crate::look::LookLutLimits;

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

/// Заготавливать ли варианты пайплайна для ВСЕХ моделей основы тона (решение 195: основа переключается на ходу).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneBasePrewarm {
    /// При создании варианта для ключа сразу создаются варианты с тремя другими моделями основы: свайп основы
    /// никогда не компилирует пайплайн посреди сессии. Стоит несколько миллисекунд CPU и память драйвера на
    /// первом кадре (`[estimate]`: цену на телефоне меряет сессия 14j).
    #[default]
    AllModels,
    /// Варианты создаются лениво, по одному, на первом кадре с такой основой (A/B-замеры стадий, тесты, где счёт
    /// вариантов важен).
    Off,
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
    /// 4×32 = 128 нитей — минимум `maxComputeWorkGroupInvocations`, который Vulkan гарантирует на любом
    /// устройстве, и кратно волне Adreno (64) и RADV (64). Узкая и высокая: выход повёрнут на 90° относительно сенсора,
    /// и нити, идущие вдоль Y выхода, читают соседние ячейки одной строки RAW (на Adreno 740 чтение пола
    /// «уровни + демозаик» 2,7 → 1,6 мс против 16×8, сессия 16a, пакет I).
    fn default() -> Self {
        Self { x: 4, y: 32 }
    }
}

/// Верхняя граница нитей в группе, которую принимает КОНФИГ (проверка без GPU): `maxComputeWorkGroupInvocations`
/// у обоих наших GPU (Adreno 740 и RADV — 1024, vkjson). Настоящий лимит устройства (нитей и размер по
/// осям, `maxComputeWorkGroupSize`) проверяется в `ViewfinderIsp::new` через `LocalSize::validate_for_device`:
/// на устройстве с меньшим лимитом (спецификация гарантирует лишь 128) — чистая ошибка, а не UB.
pub const MAX_WORKGROUP_INVOCATIONS: u32 = 1024;

/// Число вариантов продукта: 4 набора стадий (виньетирование × таблица вида) × 4 модели основы тона.
pub const DEFAULT_PIPELINE_VARIANT_CAPACITY: usize = 4 * 4;

/// Верхняя граница слотов кадров: больше двух-трёх кадров в полёте видоискателю не нужно никогда.
pub const MAX_FRAME_SLOTS: usize = 8;

/// Наибольший кадр (мозаика RAW, пиксели), на котором ISP обязан работать: под него при создании проверяется,
/// что суммы плиток статистики AWB не переполнят u32 (`AwbStatisticsSettings::check_for_mosaic`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LargestMosaicConfig {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
}

impl Default for LargestMosaicConfig {
    /// `MAX_SUPPORTED_MOSAIC` (8192×6144): вдвое больше стороны сенсора OnePlus 11.
    fn default() -> Self {
        Self {
            width: MAX_SUPPORTED_MOSAIC.width,
            height: MAX_SUPPORTED_MOSAIC.height,
        }
    }
}

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
    /// Как сводить статистику AWB внутри рабочей группы (по умолчанию — shared-память; на Adreno 740 она дешевле
    /// subgroup на ~5 мс кадра со статистикой, сессия 16a).
    pub awb_reduction: AwbReductionPolicy,
    /// Формат изображения видоискателя.
    pub storage_format: ViewfinderStorageFormat,
    /// Сколько вариантов пайплайна (набор стадий × формат × источник × модель основы) держать одновременно.
    /// По умолчанию [`DEFAULT_PIPELINE_VARIANT_CAPACITY`] (16): в продукте набор стадий принимает до 4 значений
    /// (виньетирование «пропуск» или «применить» × таблица вида есть или нет), каждое — в четырёх моделях основы
    /// (прогрев `tone_base_prewarm`); формат и источник у потока постоянны. Остальные A/B-варианты «без одной
    /// стадии» живут по вытеснению: при переполнении давно не использованный вариант уходит в отложенное уничтожение.
    pub pipeline_variant_capacity: usize,
    /// Прогрев вариантов всех моделей основы (по умолчанию включён). Прогрев занимает только свободные места кэша и
    /// никогда не вытесняет вариант, которым рисуют.
    pub tone_base_prewarm: ToneBasePrewarm,
    /// Наибольшая карта коррекции виньетирования, под которую выделяются буферы слотов. Карта больше —
    /// явная ошибка кадра (`ViewfinderError::LensShadingMap`), а не молчаливый пропуск коррекции.
    pub lens_shading_map: LensShadingMapLimits,
    /// Наибольшая таблица вида (LUT), под которую выделяются буферы слотов. Таблица больше — явная ошибка кадра
    /// (`ViewfinderError::LookLut`), а не молчаливый пропуск.
    pub look_lut: LookLutLimits,
    /// Наибольший кадр: при создании проверяется, что сетка и запас сумм AWB не переполнят u32 ни на каком кадре
    /// не больше этого (явная ошибка `ViewfinderError::AwbStatistics`, а не молчаливое переполнение на кадре).
    pub largest_mosaic: LargestMosaicConfig,
}

impl Default for ViewfinderIspConfig {
    fn default() -> Self {
        Self {
            frame_slots: 2,
            workgroup_size: WorkgroupSizeConfig::default(),
            awb_statistics: AwbStatisticsSettings::default(),
            awb_reduction: AwbReductionPolicy::default(),
            storage_format: ViewfinderStorageFormat::default(),
            pipeline_variant_capacity: DEFAULT_PIPELINE_VARIANT_CAPACITY,
            tone_base_prewarm: ToneBasePrewarm::default(),
            lens_shading_map: LensShadingMapLimits::default(),
            look_lut: LookLutLimits::default(),
            largest_mosaic: LargestMosaicConfig::default(),
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
        self.lens_shading_map.validate()?;
        self.look_lut.validate()?;
        let settings = self.awb_statistics;
        if settings.grid.columns == 0 || settings.grid.rows == 0 {
            return invalid(format!("пустая сетка статистики AWB {:?}", settings.grid));
        }
        let threshold = settings.saturation_threshold;
        if !(threshold.is_finite() && threshold > 0.0 && threshold <= 1.0) {
            return invalid(format!("порог пересвета AWB {threshold} вне (0, 1]"));
        }
        // Запас сумм и переполнение u32 на наибольшем поддерживаемом кадре — явная ошибка при запуске.
        settings.check_for_mosaic(PixelSize::new(self.largest_mosaic.width, self.largest_mosaic.height))?;
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
        assert_eq!(partial.workgroup_size, WorkgroupSizeConfig { x: 4, y: 32 });
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
