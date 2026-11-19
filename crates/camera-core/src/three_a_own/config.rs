//! Настройки нашего AE на стороне движка (сессия 08a): стартовое значение, политика
//! защиты от мерцания и пороги, чтобы не дёргать запрос по пустякам.
//!
//! Все числа — данные конфига, а не код; обоснование каждого рядом.

use serde::{Deserialize, Serialize};

use crate::config::CameraEngineConfigError;

/// Когда держать выдержку кратной полупериоду сети (защита от мерцания ламп).
///
/// Только ручной выбор: автоопределение по детектору HAL убрано (решение 77) — детектор на
/// этом телефоне фиктивный (`60HZ` почти на всех кадрах, даже под светом без мерцания).
/// **По умолчанию защита выключена**; включается вручную (`AlwaysProtect`) с частотой сети
/// 50 или 60 Гц из конфига (позже — переключатель в UI, сессия 16). Автоопределение — в
/// бэклоге (кандидат: свой детектор по построчной статистике RAW на GPU). Старый JSON
/// `{"policy":"follow_detector",...}` отвергается строгим разбором, а не молча подменяется.
///
/// В JSON — простая строка (`"flicker": "always_protect"`), а не объект: у строки не бывает лишних
/// полей, а `deny_unknown_fields` на варианты без полей у размеченного enum не действует (старый
/// `{"policy":"always_protect","clear_after_frames":12}` молча принимался бы).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlickerPolicy {
    /// Выдержка всегда кратна полупериоду сети (частота — из конфига Android, по умолчанию
    /// 50 Гц, решение 67). Отчёты HAL о мерцании (в том числе `None`) защиту не снимают.
    AlwaysProtect,
    /// Защиты нет: выдержка свободна до предела кадра. По умолчанию (решение 77).
    #[default]
    NeverProtect,
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
        Ok(())
    }
}
