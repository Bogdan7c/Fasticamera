//! Настройки нашего AE на стороне движка (сессия 08a): стартовое значение, политика
//! защиты от мерцания и пороги, чтобы не дёргать запрос по пустякам.
//!
//! Все числа — данные конфига, а не код; обоснование каждого рядом.

use serde::{Deserialize, Serialize};

use crate::config::CameraEngineConfigError;

/// Когда держать выдержку кратной полупериоду сети (защита от мерцания ламп).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case", tag = "policy")]
pub enum FlickerPolicy {
    /// Защита включена с начала потока и снимается, только когда HAL `clear_after_frames`
    /// кадров подряд говорит «мерцания нет»; любой отчёт о 50/60 Гц возвращает её сразу
    /// (решение 08a: детектор HAL). Нет ключа в результате — состояние не меняется.
    FollowDetector { clear_after_frames: u32 },
    /// Защита всегда (детектор HAL игнорируется, кроме уточнения частоты сети).
    AlwaysProtect,
    /// Защиты нет никогда (отладка, сравнение).
    NeverProtect,
}

impl Default for FlickerPolicy {
    /// 30 кадров ≈ 0,5 с при 60 к/с: достаточно, чтобы единичный «пустой» отчёт детектора не
    /// снял защиту, и достаточно мало, чтобы на улице выдержка не была зря ограничена.
    /// Это **оценка, сверить в 08c** на телефоне.
    fn default() -> Self {
        Self::FollowDetector { clear_after_frames: 30 }
    }
}

/// Настройки исполнителя `RawStatsExposure`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct OwnExposureConfig {
    /// Стартовое произведение «выдержка × ISO», с·ISO, пока нет ни предложения рендера, ни
    /// памяти прошлого потока. 10 мс × ISO 400 = 4,0 — комнатный свет. **Оценка, сверить в 08c.**
    pub initial_exposure_seconds_iso: f64,
    pub flicker: FlickerPolicy,
    /// Допуск «цель достигнута», EV: ISO целое, выдержка с шагом, поэтому точного попадания
    /// нет. 0,05 EV (~3,5 % света) ниже того, что видит глаз; больше — честная поправка
    /// `TargetNotReached`.
    pub not_reached_tolerance_ev: f32,
    /// Шаг округления сообщаемой нехватки, EV. Достаточно грубый, чтобы `ControlsAdjusted` не
    /// сыпались при дрожании цели у предела, и достаточно мелкий для экрана; ошибка
    /// `shortfall_ev` для рендера — не больше половины шага.
    pub shortfall_report_step_ev: f32,
    /// Минимальный сдвиг цели, при котором запрос перестраивается, EV. 1/48 EV — четверть
    /// шага компенсации вендорского AE (1/6 EV = 8/48): мельче шума замера, зато плавный
    /// дрейф сцены всё равно накапливается и доходит до порога.
    pub replan_min_change_ev: f32,
}

impl Default for OwnExposureConfig {
    fn default() -> Self {
        Self {
            initial_exposure_seconds_iso: 4.0,
            flicker: FlickerPolicy::default(),
            not_reached_tolerance_ev: 0.05,
            shortfall_report_step_ev: 0.1,
            replan_min_change_ev: 1.0 / 48.0,
        }
    }
}

impl OwnExposureConfig {
    /// Предел кадров «чисто» до снятия защиты: защита от опечатки в конфиге.
    pub const MAX_CLEAR_AFTER_FRAMES: u32 = 3_600;
    /// Предел допусков в EV: больше — AE превращается в случайный.
    pub const MAX_TOLERANCE_EV: f32 = 2.0;

    pub fn validate(&self) -> Result<(), CameraEngineConfigError> {
        let seed = self.initial_exposure_seconds_iso;
        if !(seed.is_finite() && seed > 0.0) {
            return Err(CameraEngineConfigError::InvalidExposureSetting {
                field: "initial_exposure_seconds_iso",
                value: seed,
            });
        }
        for (field, value) in [
            ("not_reached_tolerance_ev", self.not_reached_tolerance_ev),
            ("shortfall_report_step_ev", self.shortfall_report_step_ev),
            ("replan_min_change_ev", self.replan_min_change_ev),
        ] {
            if !(value.is_finite() && value > 0.0 && value <= Self::MAX_TOLERANCE_EV) {
                return Err(CameraEngineConfigError::InvalidExposureSetting {
                    field,
                    value: f64::from(value),
                });
            }
        }
        if let FlickerPolicy::FollowDetector { clear_after_frames } = self.flicker
            && (clear_after_frames == 0 || clear_after_frames > Self::MAX_CLEAR_AFTER_FRAMES)
        {
            return Err(CameraEngineConfigError::InvalidThreeALimit {
                field: "exposure.flicker.clear_after_frames",
                value: u64::from(clear_after_frames),
                max: u64::from(Self::MAX_CLEAR_AFTER_FRAMES),
            });
        }
        Ok(())
    }
}
