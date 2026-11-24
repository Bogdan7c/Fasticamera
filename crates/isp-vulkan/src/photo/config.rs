//! Конфиг проявления снимка: serde, `deny_unknown_fields`, значения по умолчанию с источником.

use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::LocalSize;
use serde::{Deserialize, Serialize};

use still_contract::shot::StillRotation;

use super::error::PhotoError;
use super::thumbnail::ThumbnailPlan;
use crate::lens_shading_map::LensShadingMapLimits;
use crate::look::LookLutLimits;
use crate::quality::WorkgroupSizeConfig;
use crate::quality::config::validate_workgroup;

/// Наибольшая высота полосы, которую конфиг разрешает: больше — это уже «весь кадр», а смысл полос в том,
/// чтобы память и время одного сабмита были ограничены. 4096 строк покрывают любой кадр телефона целиком
/// (проверка паритета «одна полоса = весь кадр» этим пользуется).
const MAX_SLICE_ROWS: u32 = 4096;

/// Длинная сторона миниатюры по умолчанию. `[estimate]`: кружок экрана 150–200 px при плотности ≈ 3 px/pt
/// даёт ≈ 200 px по стороне; 256 px оставляют запас на запись «глубже» и стоят ≈ 0,2 МБ памяти (решение 155).
const DEFAULT_THUMBNAIL_LONG_SIDE_PX: u32 = 256;
/// Границы `thumbnail_long_side_px`: меньше — не разглядеть, больше — это уже не «маленькая копия в обычной
/// памяти» (1024² × 4 Б = 4 МБ, а экрану нужно лишь на кружок).
const THUMBNAIL_LONG_SIDE_RANGE: std::ops::RangeInclusive<u32> = 16..=1024;

/// Размер рабочей группы проходов снимка по умолчанию (уровни, 8 проходов Menon, вывод): 128×2. `[measured]` на
/// Adreno 740 (сессия 16a): сумма проходов полосы 7,14 мс против 7,85 мс у 16×16 (−9 %), картинка бит-в-бит;
/// группа идёт вдоль строк плоскостей (128 нитей подряд по X), как у видоискателя.
const DEFAULT_WORKGROUP_SIZE: WorkgroupSizeConfig = WorkgroupSizeConfig { x: 128, y: 2 };

/// Настройки проявления снимка.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PhotoConfig {
    /// Высота полосы проявления, строки: чётная, 2…4096. Чем полоса ниже, тем короче один сабмит (видоискатель
    /// не стоит) и меньше память плоскостей, но тем больше доля ореола, который обсчитывается дважды (12 строк
    /// сверху и снизу при 256 строках полосы — 9 %).
    pub slice_rows: u32,
    /// Размер рабочей группы проходов полосы (уровни, Menon, вывод). Миниатюра от него НЕ зависит: у неё своя
    /// фиксированная группа (`thumbnail_pass`), потому что число нитей задаёт порядок сложения в её редукции.
    pub workgroup_size: WorkgroupSizeConfig,
    /// Длинная сторона миниатюры снимка, пиксели: 16…1024 (решение 155). Миниатюра — маленькая копия для экрана
    /// камеры (кружок в углу), уменьшается на GPU внутри последней полосы проявки и уходит в обычную память.
    pub thumbnail_long_side_px: u32,
    /// Наибольшая карта коррекции виньетирования (буфер на проявку). Карта больше — `begin` отказывает.
    pub lens_shading_map: LensShadingMapLimits,
    /// Наибольшая таблица вида (LUT; буфер на проявку). Таблица больше — `begin` отказывает.
    pub look_lut: LookLutLimits,
}

impl Default for PhotoConfig {
    /// 256 строк — ОЦЕНКА, не замер: полоса 4096×256 — 1 Мпикс, около 1/12 кадра 12 Мпикс; на ПК (RADV) один
    /// сабмит такой полосы короче кадра видоискателя с большим запасом (см. отчёт пакета). Подбор под
    /// Adreno 740 — замером на телефоне (DVFS растягивает работу, мс GPU там не цена).
    fn default() -> Self {
        Self {
            slice_rows: 256,
            workgroup_size: DEFAULT_WORKGROUP_SIZE,
            thumbnail_long_side_px: DEFAULT_THUMBNAIL_LONG_SIDE_PX,
            lens_shading_map: LensShadingMapLimits::default(),
            look_lut: LookLutLimits::default(),
        }
    }
}

impl PhotoConfig {
    /// Проверяет весь конфиг (до создания объектов Vulkan).
    pub fn validate(&self) -> Result<(), PhotoError> {
        if self.slice_rows < 2 || self.slice_rows > MAX_SLICE_ROWS || !self.slice_rows.is_multiple_of(2) {
            return Err(PhotoError::InvalidConfig {
                what: format!(
                    "slice_rows {}: нужно чётное число 2..={MAX_SLICE_ROWS} (полоса начинается с ячейки CFA 2x2)",
                    self.slice_rows
                ),
            });
        }
        if !THUMBNAIL_LONG_SIDE_RANGE.contains(&self.thumbnail_long_side_px) {
            return Err(PhotoError::InvalidConfig {
                what: format!(
                    "thumbnail_long_side_px {}: нужно {}..={}",
                    self.thumbnail_long_side_px,
                    THUMBNAIL_LONG_SIDE_RANGE.start(),
                    THUMBNAIL_LONG_SIDE_RANGE.end()
                ),
            });
        }
        self.local_size()?;
        self.lens_shading_map.validate()?;
        self.look_lut.validate()?;
        Ok(())
    }

    /// План миниатюры снимка `image_size` с поворотом `rotation` при этом конфиге.
    pub fn thumbnail_plan(&self, image_size: PixelSize, rotation: StillRotation) -> Result<ThumbnailPlan, PhotoError> {
        ThumbnailPlan::new(image_size, rotation, self.thumbnail_long_side_px)
    }

    /// Размер рабочей группы как `LocalSize` (ноль и больше 1024 нитей — ошибка конфига).
    pub fn local_size(&self) -> Result<LocalSize, PhotoError> {
        Ok(validate_workgroup(self.workgroup_size)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip_through_json() {
        let config = PhotoConfig::default();
        assert!(config.validate().is_ok());
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<PhotoConfig>(&json).unwrap(), config);
        let partial: PhotoConfig = serde_json::from_str(r#"{"slice_rows": 64}"#).unwrap();
        assert_eq!(partial.slice_rows, 64);
        assert_eq!(partial.workgroup_size, DEFAULT_WORKGROUP_SIZE);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<PhotoConfig>(r#"{"slice_row": 64}"#).is_err());
    }

    #[test]
    fn odd_zero_and_oversized_slice_rows_are_config_errors() {
        for slice_rows in [0, 1, 3, 255, 4098, 8192] {
            let config = PhotoConfig {
                slice_rows,
                ..PhotoConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(PhotoError::InvalidConfig { .. })),
                "slice_rows {slice_rows}"
            );
        }
        for slice_rows in [2, 4, 4096] {
            let config = PhotoConfig {
                slice_rows,
                ..PhotoConfig::default()
            };
            assert!(config.validate().is_ok(), "slice_rows {slice_rows}");
        }
    }

    #[test]
    fn thumbnail_long_side_is_range_checked_and_defaults_to_256() {
        assert_eq!(PhotoConfig::default().thumbnail_long_side_px, 256);
        for thumbnail_long_side_px in [0, 15, 1025, 4096] {
            let config = PhotoConfig {
                thumbnail_long_side_px,
                ..PhotoConfig::default()
            };
            assert!(
                matches!(config.validate(), Err(PhotoError::InvalidConfig { .. })),
                "thumbnail_long_side_px {thumbnail_long_side_px}"
            );
        }
        for thumbnail_long_side_px in [16, 256, 1024] {
            let config = PhotoConfig {
                thumbnail_long_side_px,
                ..PhotoConfig::default()
            };
            assert!(
                config.validate().is_ok(),
                "thumbnail_long_side_px {thumbnail_long_side_px}"
            );
        }
        let partial: PhotoConfig = serde_json::from_str(r#"{"thumbnail_long_side_px": 128}"#).unwrap();
        let plan = partial
            .thumbnail_plan(PixelSize::new(4000, 3000), StillRotation::None)
            .unwrap();
        assert_eq!(plan.size(), PixelSize::new(128, 96));
    }

    #[test]
    fn broken_workgroup_is_a_config_error() {
        let config = PhotoConfig {
            workgroup_size: WorkgroupSizeConfig { x: 64, y: 32 },
            ..PhotoConfig::default()
        };
        assert!(matches!(config.validate(), Err(PhotoError::InvalidConfig { .. })));
    }
}
