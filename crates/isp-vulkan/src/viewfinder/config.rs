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
use super::variant_family::VARIANTS_PER_STAGE_SET;
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

/// Как заготавливать родственные варианты пайплайна (решение 195 + пакеты I5 и I6 сессии 16a).
///
/// Вариант шейдера зависит от осей, которые переключаются на ходу: модель основы тона (свайп вида), кадр со статистикой
/// AWB или без (каждый 4-й кадр) и раскладка групп по плиткам статистики (плавный зум). Что именно входит в семейство,
/// решает один модуль (`variant_family`); здесь только способ. Заготовка занимает только свободные места кэша.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantPrewarm {
    /// Поток рендера создаёт только вариант первого кадра; остальное семейство компилирует фоновый поток, и оно
    /// подхватывается между кадрами. Если кадру нужен вариант, который фон ещё не выдал, кадр компилирует его сам или
    /// ждёт текущую компиляцию фона (см. `background_prewarm`). Если ОС не даст поток — заготовка идёт на потоке
    /// рендера (с громким журналом). Старые имена конфига (`all_related`, `all_models`) читаются как этот способ.
    #[default]
    #[serde(alias = "all_related", alias = "all_models")]
    InBackground,
    /// Всё семейство создаётся на потоке рендера в первом кадре (на Adreno 740 — 1,5–1,8 с остановки при каждом запуске).
    /// Нужно тестам, где важна детерминированность без ожидания фона.
    OnRenderThread,
    /// Варианты создаются лениво, по одному, на первом кадре, которому они нужны (A/B-замеры стадий, тесты, где счёт
    /// вариантов важен).
    Off,
}

/// Предел ожидания кадром фоновой компиляции нужного варианта, мс (с запасом на самую медленную компиляцию).
pub const DEFAULT_BACKGROUND_WAIT_LIMIT_MS: u32 = 1000;

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

/// Наборов остальных стадий в продукте: виньетирование («пропуск» или «применить») × таблица вида (есть или нет).
const PRODUCT_STAGE_SETS: usize = 2 * 2;

/// Ёмкость кэша вариантов по умолчанию: ВСЕ варианты, которыми продукт пользуется по очереди, без вытеснения.
///
/// Выводится из перечислений, а не задана числом: набор стадий (4) × класс статистики AWB (без статистики + каждая
/// раскладка групп по плиткам: сейчас 3, потому что кадр со статистикой идёт каждый 4-й, а зум переключает
/// раскладку) × модель основы тона (4) = 48. Добавление модели основы или раскладки меняет число само.
pub const DEFAULT_PIPELINE_VARIANT_CAPACITY: usize = PRODUCT_STAGE_SETS * VARIANTS_PER_STAGE_SET;

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
    /// Сколько вариантов пайплайна (набор стадий × формат × источник × класс статистики × модель основы) держать
    /// одновременно. По умолчанию [`DEFAULT_PIPELINE_VARIANT_CAPACITY`] (48): в продукте набор стадий принимает до 4
    /// значений (виньетирование × таблица вида), каждое — в трёх классах статистики AWB (без неё, со статистикой с
    /// короткой и с общей раскладкой) и четырёх моделях основы; формат и источник у потока постоянны. Остальные
    /// A/B-варианты «без одной стадии» живут по вытеснению: при переполнении самый давно использованный вариант
    /// (заготовленные и ни разу не использованные — первыми) уходит в отложенное уничтожение.
    pub pipeline_variant_capacity: usize,
    /// Заготовка родственных вариантов (по умолчанию включена). Она занимает только свободные места кэша и
    /// никогда не вытесняет вариант, которым рисуют.
    #[serde(alias = "tone_base_prewarm")]
    pub variant_prewarm: VariantPrewarm,
    /// Сколько миллисекунд кадр готов ждать вариант, который фон компилирует прямо сейчас (одна компиляция — 150–200 мс
    /// на Adreno 740). Не уложился — кадр компилирует сам.
    pub background_wait_limit_ms: u32,
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
            variant_prewarm: VariantPrewarm::default(),
            background_wait_limit_ms: DEFAULT_BACKGROUND_WAIT_LIMIT_MS,
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
    fn old_prewarm_names_still_read_as_background_prewarm() {
        for name in ["all_related", "all_models", "in_background"] {
            let text = format!(r#"{{"variant_prewarm": "{name}"}}"#);
            let parsed: ViewfinderIspConfig = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed.variant_prewarm, VariantPrewarm::InBackground, "{name}");
        }
        let old_key: ViewfinderIspConfig = serde_json::from_str(r#"{"tone_base_prewarm": "all_models"}"#).unwrap();
        assert_eq!(old_key.variant_prewarm, VariantPrewarm::InBackground);
        let on_thread: ViewfinderIspConfig =
            serde_json::from_str(r#"{"variant_prewarm": "on_render_thread"}"#).unwrap();
        assert_eq!(on_thread.variant_prewarm, VariantPrewarm::OnRenderThread);
        assert_eq!(
            ViewfinderIspConfig::default().variant_prewarm,
            VariantPrewarm::InBackground
        );
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
