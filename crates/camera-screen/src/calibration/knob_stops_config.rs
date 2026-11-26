//! Вид упоров на шкалах теплоты и оттенка (решения 288, 293): риска-упор, тусклый ход за ним и тусклое число.
//!
//! Числа — `[estimate]`: подбор на телефоне. Где стоит упор, считает рендер (`look_contract::white_stops`); здесь
//! только то, КАК это рисуется.

use serde::{Deserialize, Serialize};

use super::widgets_config::CalibrationWidgetsConfigError;

/// Вид упоров ручек теплоты и оттенка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct KnobStopsLookConfig {
    /// Толщина риски-упора вдоль линейки, pt. `[estimate]`.
    pub tick_thickness_pt: f64,
    /// Длина риски-упора поперёк линейки, pt: длиннее обычной (13 pt), чтобы читалась среди рисок. `[estimate]`.
    pub tick_length_pt: f64,
    /// Прозрачность риски-упора (белая). `[estimate]`.
    pub tick_alpha: f32,
    /// Во сколько раз тусклее риски хода за упором (`0..=1`). `[estimate]`.
    pub beyond_tick_dim_factor: f32,
    /// Прозрачность числа значения, пока ручка стоит за упором (картинка дальше упора не меняется). `[estimate]`.
    pub beyond_value_alpha: f32,
}

impl Default for KnobStopsLookConfig {
    fn default() -> Self {
        Self {
            tick_thickness_pt: 3.0,
            tick_length_pt: 22.0,
            tick_alpha: 0.9,
            beyond_tick_dim_factor: 0.35,
            beyond_value_alpha: 0.45,
        }
    }
}

impl KnobStopsLookConfig {
    /// Проверка значений: размеры конечные и больше нуля, доли в `0..=1`.
    pub fn validate(&self) -> Result<(), CalibrationWidgetsConfigError> {
        for (field, value) in [
            ("knob_stops.tick_thickness_pt", self.tick_thickness_pt),
            ("knob_stops.tick_length_pt", self.tick_length_pt),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(CalibrationWidgetsConfigError::NonPositive { field, value });
            }
        }
        for (field, value) in [
            ("knob_stops.tick_alpha", f64::from(self.tick_alpha)),
            (
                "knob_stops.beyond_tick_dim_factor",
                f64::from(self.beyond_tick_dim_factor),
            ),
            ("knob_stops.beyond_value_alpha", f64::from(self.beyond_value_alpha)),
        ] {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                return Err(CalibrationWidgetsConfigError::FractionOutOfRange { field, value });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_each_bad_number_is_named_in_the_error() {
        assert_eq!(KnobStopsLookConfig::default().validate(), Ok(()));
        let zero_length = KnobStopsLookConfig {
            tick_length_pt: 0.0,
            ..KnobStopsLookConfig::default()
        };
        assert!(matches!(
            zero_length.validate(),
            Err(CalibrationWidgetsConfigError::NonPositive {
                field: "knob_stops.tick_length_pt",
                ..
            })
        ));
        let loud_value = KnobStopsLookConfig {
            beyond_value_alpha: 1.5,
            ..KnobStopsLookConfig::default()
        };
        assert!(matches!(
            loud_value.validate(),
            Err(CalibrationWidgetsConfigError::FractionOutOfRange {
                field: "knob_stops.beyond_value_alpha",
                ..
            })
        ));
        let not_a_number = KnobStopsLookConfig {
            beyond_tick_dim_factor: f32::NAN,
            ..KnobStopsLookConfig::default()
        };
        assert!(not_a_number.validate().is_err());
    }
}
