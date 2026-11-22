//! Конфиг проявления снимка: serde, `deny_unknown_fields`, значения по умолчанию с источником.

use gpu_vulkan::compute::LocalSize;
use serde::{Deserialize, Serialize};

use super::error::PhotoError;
use crate::quality::WorkgroupSizeConfig;
use crate::quality::config::validate_workgroup;

/// Наибольшая высота полосы, которую конфиг разрешает: больше — это уже «весь кадр», а смысл полос в том,
/// чтобы память и время одного сабмита были ограничены. 4096 строк покрывают любой кадр телефона целиком
/// (проверка паритета «одна полоса = весь кадр» этим пользуется).
const MAX_SLICE_ROWS: u32 = 4096;

/// Настройки проявления снимка.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PhotoConfig {
    /// Высота полосы проявления, строки: чётная, 2…4096. Чем полоса ниже, тем короче один сабмит (видоискатель
    /// не стоит) и меньше память плоскостей, но тем больше доля ореола, который обсчитывается дважды (12 строк
    /// сверху и снизу при 256 строках полосы — 9 %).
    pub slice_rows: u32,
    /// Размер рабочей группы всех проходов (как у стенда V4).
    pub workgroup_size: WorkgroupSizeConfig,
}

impl Default for PhotoConfig {
    /// 256 строк — ОЦЕНКА, не замер: полоса 4096×256 — 1 Мпикс, около 1/12 кадра 12 Мпикс; на ПК (RADV) один
    /// сабмит такой полосы короче кадра видоискателя с большим запасом (см. отчёт пакета). Подбор под
    /// Adreno 740 — замером на телефоне (DVFS растягивает работу, мс GPU там не цена).
    fn default() -> Self {
        Self {
            slice_rows: 256,
            workgroup_size: WorkgroupSizeConfig::default(),
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
        self.local_size()?;
        Ok(())
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
        assert_eq!(partial.workgroup_size, WorkgroupSizeConfig::default());
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
    fn broken_workgroup_is_a_config_error() {
        let config = PhotoConfig {
            workgroup_size: WorkgroupSizeConfig { x: 64, y: 32 },
            ..PhotoConfig::default()
        };
        assert!(matches!(config.validate(), Err(PhotoError::InvalidConfig { .. })));
    }
}
