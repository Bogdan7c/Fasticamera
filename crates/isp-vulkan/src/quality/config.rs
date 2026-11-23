//! Конфиг стенда «запас под видео» (V4): serde, `deny_unknown_fields`, значения по умолчанию с источником.

use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use serde::{Deserialize, Serialize};

use super::error::QualityError;
use crate::lens_shading_map::LensShadingMapLimits;
use crate::look::LookLutLimits;

/// Наибольшее число нитей в рабочей группе, которое конфиг разрешает (проверка без GPU). 1024 —
/// `maxComputeWorkGroupInvocations` и Adreno 740 (vkjson телефона), и RADV; спецификация гарантирует лишь
/// 128, поэтому лимиты устройства проверяются ещё раз в `VideoHeadroomWorkload::new`
/// (`LocalSize::validate_for_device`).
const MAX_WORKGROUP_INVOCATIONS: u32 = 1024;
/// Больше слотов кадров стенду не нужно (обычно 2: кадр k на GPU, кадр k+1 записывается).
const MAX_FRAME_SLOTS: usize = 8;

/// Формат выхода (цели приёмника кадра).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityOutputFormat {
    /// 10 бит UNORM, если устройство умеет писать его как storage image (как вход кодера из 00), иначе fp16.
    #[default]
    PreferTenBitUnorm,
    /// Только fp16 (паритетные тесты, отладка).
    HalfFloat,
}

impl QualityOutputFormat {
    /// Политика формата `StorageImage` для приёмника кадра.
    pub fn storage_policy(self) -> StorageFormatPolicy {
        match self {
            Self::PreferTenBitUnorm => StorageFormatPolicy::PreferTenBitUnorm,
            Self::HalfFloat => StorageFormatPolicy::HalfFloatOnly,
        }
    }
}

/// Размер рабочей группы всех проходов.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkgroupSizeConfig {
    /// Нитей по X.
    pub x: u32,
    /// Нитей по Y.
    pub y: u32,
}

impl Default for WorkgroupSizeConfig {
    /// 16×16 = 256 нитей: кратно размеру subgroup Adreno (64/128) и RADV (64), строки по 64 байта f32.
    fn default() -> Self {
        Self { x: 16, y: 16 }
    }
}

/// Настройки временного шумодава «на пробу» (формулы — `shaders/quality_temporal_denoise.comp`).
///
/// Значения по умолчанию — оценка для стенда, а не калибровка: модель шума сенсора (профиль C7 из 00)
/// подставится в плане `video`, вместе с компенсацией движения.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalDenoiseConfig {
    /// Включён ли шумодав (выключенный — проход не записывается вовсе, его время = 0).
    pub enabled: bool,
    /// Наибольший вес истории, 0…1 (не включая 1). 0,8 — в установившемся режиме дисперсия шума
    /// неподвижной сцены падает в (1 − w)/(1 + w) = 1/9 раза, то есть σ втрое.
    pub strength: f32,
    /// Порог движения T в единицах σ шума: средняя по каналам разность ≥ T·σ — движение, история
    /// отбрасывается; от T/2 до T вес плавно спадает.
    pub motion_threshold_sigmas: f32,
    /// σ шума в чёрном (шум чтения), в нормированном свете 0…1 (> 0). 0,002 ≈ 2 единицы RAW10 после уровней.
    pub noise_floor: f32,
    /// Дробовой шум: прибавка к дисперсии на единицу сигнала (≥ 0).
    pub shot_noise_gain: f32,
}

impl Default for TemporalDenoiseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strength: 0.8,
            motion_threshold_sigmas: 3.0,
            noise_floor: 0.002,
            shot_noise_gain: 0.0004,
        }
    }
}

impl TemporalDenoiseConfig {
    /// Проверяет диапазоны (до создания объектов Vulkan).
    pub fn validate(&self) -> Result<(), QualityError> {
        let invalid = |what: String| Err(QualityError::InvalidConfig { what });
        if !(self.strength.is_finite() && (0.0..1.0).contains(&self.strength)) {
            return invalid(format!("strength {} вне [0, 1)", self.strength));
        }
        if !(self.motion_threshold_sigmas.is_finite() && self.motion_threshold_sigmas > 0.0) {
            return invalid(format!(
                "motion_threshold_sigmas {} должен быть > 0",
                self.motion_threshold_sigmas
            ));
        }
        if !(self.noise_floor.is_finite() && self.noise_floor > 0.0) {
            return invalid(format!("noise_floor {} должен быть > 0", self.noise_floor));
        }
        if !(self.shot_noise_gain.is_finite() && self.shot_noise_gain >= 0.0) {
            return invalid(format!("shot_noise_gain {} должен быть ≥ 0", self.shot_noise_gain));
        }
        Ok(())
    }
}

/// Конфиг стенда V4.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VideoHeadroomConfig {
    /// Слотов кадров (наборы дескрипторов, uniform-параметры, метки времени): 1…8.
    pub frame_slots: usize,
    /// Формат выхода (для создания приёмника кадра).
    pub output_format: QualityOutputFormat,
    /// Размер рабочей группы.
    pub workgroup_size: WorkgroupSizeConfig,
    /// Временной шумодав.
    pub temporal_denoise: TemporalDenoiseConfig,
    /// Наибольшая карта коррекции виньетирования (буфер на слот кадра). Карта больше — ошибка кадра.
    pub lens_shading_map: LensShadingMapLimits,
    /// Наибольшая таблица вида (LUT; буфер на слот кадра). Таблица больше — ошибка кадра.
    pub look_lut: LookLutLimits,
}

impl Default for VideoHeadroomConfig {
    fn default() -> Self {
        Self {
            frame_slots: 2,
            output_format: QualityOutputFormat::default(),
            workgroup_size: WorkgroupSizeConfig::default(),
            temporal_denoise: TemporalDenoiseConfig::default(),
            lens_shading_map: LensShadingMapLimits::default(),
            look_lut: LookLutLimits::default(),
        }
    }
}

impl VideoHeadroomConfig {
    /// Проверяет весь конфиг.
    pub fn validate(&self) -> Result<(), QualityError> {
        if self.frame_slots == 0 || self.frame_slots > MAX_FRAME_SLOTS {
            return Err(QualityError::InvalidConfig {
                what: format!("frame_slots {} вне 1..={MAX_FRAME_SLOTS}", self.frame_slots),
            });
        }
        self.local_size()?;
        self.lens_shading_map.validate()?;
        self.look_lut.validate()?;
        self.temporal_denoise.validate()
    }

    /// Размер рабочей группы как `LocalSize` (ноль и больше 1024 нитей — ошибка конфига).
    pub fn local_size(&self) -> Result<LocalSize, QualityError> {
        validate_workgroup(self.workgroup_size)
    }
}

/// Проверяет размер рабочей группы.
pub(crate) fn validate_workgroup(size: WorkgroupSizeConfig) -> Result<LocalSize, QualityError> {
    let invocations = u64::from(size.x) * u64::from(size.y);
    if invocations > u64::from(MAX_WORKGROUP_INVOCATIONS) {
        return Err(QualityError::InvalidConfig {
            what: format!(
                "рабочая группа {}x{} больше {MAX_WORKGROUP_INVOCATIONS} нитей",
                size.x, size.y
            ),
        });
    }
    LocalSize::new(size.x, size.y).map_err(|error| QualityError::InvalidConfig {
        what: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip_through_json() {
        let config = VideoHeadroomConfig::default();
        assert!(config.validate().is_ok());
        let json = serde_json::to_string(&config).unwrap();
        let parsed: VideoHeadroomConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
        // Частичный конфиг дополняется значениями по умолчанию.
        let partial: VideoHeadroomConfig = serde_json::from_str(r#"{"frame_slots": 3}"#).unwrap();
        assert_eq!(partial.frame_slots, 3);
        assert_eq!(partial.temporal_denoise, TemporalDenoiseConfig::default());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<VideoHeadroomConfig>(r#"{"frame_slot": 2}"#).is_err());
        let typo = r#"{"temporal_denoise": {"enabled": true, "strenght": 0.5, "motion_threshold_sigmas": 3.0,
            "noise_floor": 0.002, "shot_noise_gain": 0.0}}"#;
        assert!(serde_json::from_str::<VideoHeadroomConfig>(typo).is_err());
    }

    #[test]
    fn out_of_range_values_are_config_errors() {
        let no_slots = VideoHeadroomConfig {
            frame_slots: 0,
            ..VideoHeadroomConfig::default()
        };
        assert!(matches!(no_slots.validate(), Err(QualityError::InvalidConfig { .. })));
        for (x, y) in [(64, 32), (0, 8)] {
            let workgroup = VideoHeadroomConfig {
                workgroup_size: WorkgroupSizeConfig { x, y },
                ..VideoHeadroomConfig::default()
            };
            assert!(workgroup.validate().is_err(), "рабочая группа {x}x{y}");
        }
        for broken in [
            TemporalDenoiseConfig {
                strength: 1.0,
                ..TemporalDenoiseConfig::default()
            },
            TemporalDenoiseConfig {
                strength: f32::NAN,
                ..TemporalDenoiseConfig::default()
            },
            TemporalDenoiseConfig {
                motion_threshold_sigmas: 0.0,
                ..TemporalDenoiseConfig::default()
            },
            TemporalDenoiseConfig {
                noise_floor: 0.0,
                ..TemporalDenoiseConfig::default()
            },
            TemporalDenoiseConfig {
                shot_noise_gain: -1.0,
                ..TemporalDenoiseConfig::default()
            },
        ] {
            assert!(broken.validate().is_err(), "{broken:?}");
        }
    }

    #[test]
    fn output_format_maps_to_storage_policy() {
        assert_eq!(
            QualityOutputFormat::PreferTenBitUnorm.storage_policy(),
            StorageFormatPolicy::PreferTenBitUnorm
        );
        assert_eq!(
            QualityOutputFormat::HalfFloat.storage_policy(),
            StorageFormatPolicy::HalfFloatOnly
        );
    }
}
