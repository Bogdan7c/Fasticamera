//! Округление остаточной подтяжки `k` до шага и предела (общее для видоискателя и кадра со вспышкой, 14d).
//!
//! Видоискатель считает подтяжку `k` и округляет её до `residual_gain_step_ev` (ключ кэша параметров ISP:
//! без округления каждый кадр пересчитывал бы параметры), не выходя за предел контроллера. Кадр со вспышкой
//! проявляется той же подтяжкой по тем же правилам — число и правила в одном месте, а не копия.

use isp_model::exposure::ExposureGain;

use crate::config::AutoExposureConfig;

/// Шаг и предел остаточной подтяжки (из конфига AE).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResidualGainLimits {
    /// Шаг округления, EV (`AutoExposureConfig::residual_gain_step_ev`).
    pub step_ev: f32,
    /// Предел по модулю, EV (`ExposureControllerConfig::max_residual_ev`).
    pub max_residual_ev: f32,
}

impl ResidualGainLimits {
    /// Берёт числа из конфига AE видоискателя.
    pub fn of(config: &AutoExposureConfig) -> Self {
        Self {
            step_ev: config.residual_gain_step_ev,
            max_residual_ev: config.controller.max_residual_ev,
        }
    }

    /// Подтяжка `gain_ev`, округлённая до шага и не выше предела. Меньше половины шага — ровно `UNITY`.
    /// Округление не выводит подтяжку за предел (шаг крупнее предела: 0,3 при 0,5 дало бы 0,6).
    pub fn quantize_ev(self, gain_ev: f32) -> ExposureGain {
        // Шаг проверен конфигом (конечный, > 0): округление конечно.
        let steps = (gain_ev / self.step_ev).round();
        ExposureGain::from_ev((steps * self.step_ev).clamp(-self.max_residual_ev, self.max_residual_ev))
            .unwrap_or(ExposureGain::UNITY)
    }
}
