//! Пределы белой: прижим значения к диапазону конфига с видимой поправкой.
//!
//! Чистая функция без состояния: владелец белого зовёт её на каждом входе (ручная команда, ответ источника,
//! запомненное значение), а вызывающий видит прижим как `ControlAdjustment`, а не молчаливое обрезание.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};

use crate::three_a_status::{AdjustmentReason, ControlAdjustment, ControlParameter};
use crate::white::WhiteConfig;

/// Значение белой после прижима и список поправок (пусто — значение применено как есть).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ClampedWhite {
    pub(super) setting: WhiteBalanceSetting,
    pub(super) adjustments: Vec<ControlAdjustment>,
}

/// Прижать `requested` к пределам конфига.
///
/// Если прижатое значение не собирается в допустимый тип (невозможно при проверенном конфиге), берётся запасной
/// свет и об этом пишется в журнал: белая всегда остаётся валидной.
pub(super) fn clamp_white(config: &WhiteConfig, requested: WhiteBalanceSetting) -> ClampedWhite {
    let requested_kelvin = requested.temperature.kelvin();
    let requested_duv = requested.tint.duv();
    let kelvin = requested_kelvin.clamp(config.min_kelvin, config.max_kelvin);
    let duv = requested_duv.clamp(-config.max_abs_duv, config.max_abs_duv);

    let mut adjustments = Vec::new();
    if kelvin != requested_kelvin {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::WhiteBalanceTemperature,
            requested: requested_kelvin,
            applied: kelvin,
            reason: if kelvin > requested_kelvin {
                AdjustmentReason::BelowWhiteLimit
            } else {
                AdjustmentReason::AboveWhiteLimit
            },
        });
    }
    if duv != requested_duv {
        adjustments.push(ControlAdjustment {
            parameter: ControlParameter::WhiteBalanceTint,
            requested: requested_duv,
            applied: duv,
            reason: if duv > requested_duv {
                AdjustmentReason::BelowWhiteLimit
            } else {
                AdjustmentReason::AboveWhiteLimit
            },
        });
    }
    if adjustments.is_empty() {
        return ClampedWhite {
            setting: requested,
            adjustments,
        };
    }
    match (ColorTemperature::from_kelvin(kelvin), Tint::from_duv(duv)) {
        (Ok(temperature), Ok(tint)) => ClampedWhite {
            setting: WhiteBalanceSetting { temperature, tint },
            adjustments,
        },
        (temperature, tint) => {
            tracing::error!(
                target: super::super::LOG_TARGET,
                ?temperature,
                ?tint,
                "прижатая белая не собралась в допустимое значение, взят запасной свет"
            );
            ClampedWhite {
                setting: config.fallback_setting_or_daylight(),
                adjustments,
            }
        }
    }
}
