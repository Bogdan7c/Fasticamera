//! Шкала ручек на экране: целые числа −N…+N (решения 234, 251; N из конфига, по умолчанию 200), где значение
//! ручки = число / N.
//!
//! Владелец шкалы — этот тип: он единственный знает, как число на линейке (`+30`) связано со значением
//! [`LookKnob`] (`0.15`) и как оно подписывается. Хранение не меняется (−1…+1), меняется только то, что видит и
//! ставит палец. Линейка (`ui-kit`) работает в тех же целых шагах: шаг значения 1/N, положение = номер шага.
//!
//! # Представление значения
//! Значение, поставленное пальцем, обязано быть РОВНО `n / N` в `f32` — ближайшим к числу `n / N`, как у литерала
//! `0.15` в коде и в файле пресета. Поэтому оно строится делением целого на целое в `f32` (деление в IEEE даёт
//! ближайшее представимое), а не умножением шага `n × 0,01` (оно давало бы `0.14999999`-подобные хвосты) и не
//! округлением `f64` → `f32` значения линейки, которое могло бы разойтись с литералом пресета в последнем разряде.

use std::num::NonZeroU32;

use look_contract::{LookKnob, LookKnobError};
use ui_kit::tokens::{PointsPerUnit, ScaleDensity};

use crate::calibration::CalibrationWidgetsConfig;
use crate::widget::error::WidgetError;

/// Шкала ручек: сколько целых шагов в значении 1,0 и сколько pt пальца приходится на один шаг.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KnobScale {
    units_per_full_scale: NonZeroU32,
    /// Путь пальца на единицу: ход / (2·N) из конфига экрана (решение 251), а не глобальный токен ui-kit.
    points_per_unit: PointsPerUnit,
}

impl KnobScale {
    /// Шкала по конфигу вида раздела.
    pub(super) fn from_config(config: &CalibrationWidgetsConfig) -> Result<Self, WidgetError> {
        let units_per_full_scale = NonZeroU32::new(config.knob_units_per_full_scale)
            .ok_or_else(|| WidgetError::Inconsistent("в шкале ручки ноль единиц на 1,0".to_owned()))?;
        let points_per_unit = PointsPerUnit::new(config.knob_points_per_unit())
            .map_err(|error| WidgetError::Inconsistent(format!("путь пальца на единицу шкалы ручки: {error}")))?;
        Ok(Self {
            units_per_full_scale,
            points_per_unit,
        })
    }

    /// Плотность линейки ui-kit: явное число pt на единицу из конфига экрана.
    pub(super) fn density(self) -> ScaleDensity {
        ScaleDensity::Explicit(self.points_per_unit)
    }

    /// Шаг значения линейки: значение ручки на одну единицу шкалы (для модели линейки `ui-kit`, в `f64`).
    pub(super) fn step(self) -> f64 {
        1.0 / f64::from(self.units_per_full_scale.get())
    }

    /// Целое число шкалы по значению ручки (ближайшее; значение между единицами, например из пресета, округляется
    /// только в подписи).
    pub(super) fn units_of(self, knob: LookKnob) -> i32 {
        (knob.get() * self.units_per_full_scale.get() as f32).round() as i32
    }

    /// Значение ручки по значению линейки (`номер шага × шаг`): ровно `n / N`, без хвостов `f32`.
    pub(super) fn knob_at(self, ruler_value: f64) -> Result<LookKnob, LookKnobError> {
        if !ruler_value.is_finite() {
            return LookKnob::new(f32::NAN);
        }
        let units = (ruler_value / self.step()).round() as i32;
        LookKnob::new(units as f32 / self.units_per_full_scale.get() as f32)
    }

    /// Подпись значения: `+30`, `−7` (минус U+2212), `0`. Значение, округлённое до нуля, без знака.
    pub(super) fn label(self, knob: LookKnob) -> String {
        debug_assert!(
            knob.get().is_finite(),
            "значение ручки в подписи должно быть конечным: {}",
            knob.get()
        );
        let units = self.units_of(knob);
        match units {
            0 => "0".to_owned(),
            positive if positive > 0 => format!("+{positive}"),
            negative => format!("\u{2212}{}", negative.unsigned_abs()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Шкала по умолчанию конфига: −200…+200 на 400 pt.
    fn scale() -> KnobScale {
        KnobScale::from_config(&CalibrationWidgetsConfig::default()).expect("умолчания допустимы")
    }

    #[test]
    fn scale_takes_units_and_finger_path_from_the_config() {
        let scale = scale();
        assert_eq!(scale.units_per_full_scale.get(), 200);
        assert_eq!(scale.step(), 0.005);
        assert_eq!(
            scale.density(),
            ScaleDensity::Explicit(PointsPerUnit::new(1.0).expect("1 pt"))
        );
        let config = CalibrationWidgetsConfig {
            knob_units_per_full_scale: 100,
            knob_full_travel_pt: 800.0,
            ..CalibrationWidgetsConfig::default()
        };
        let other = KnobScale::from_config(&config).expect("допустимо");
        assert_eq!(other.step(), 0.01);
        assert_eq!(
            other.density(),
            ScaleDensity::Explicit(PointsPerUnit::new(4.0).expect("4 pt"))
        );
    }

    #[test]
    fn label_has_sign_integer_and_a_true_minus() {
        let scale = scale();
        let label = |value: f32| scale.label(LookKnob::new(value).expect("в диапазоне"));
        assert_eq!(label(0.15), "+30");
        assert_eq!(label(-0.035), "\u{2212}7");
        assert_eq!(label(0.0), "0");
        assert_eq!(label(-0.0), "0");
        assert_eq!(label(1.0), "+200");
        assert_eq!(label(-1.0), "\u{2212}200");
        assert_eq!(label(-0.002), "0", "значение, округлённое до нуля, без знака");
        assert_eq!(label(0.3), "+60");
    }

    #[test]
    fn every_integer_unit_becomes_exactly_n_over_200_without_f32_tails() {
        let scale = scale();
        for units in -200_i32..=200 {
            // Значение линейки считается как `номер шага × шаг` в f64: именно так его отдаёт модель.
            let ruler_value = f64::from(units) * scale.step();
            let knob = scale.knob_at(ruler_value).expect("значение в диапазоне");
            assert_eq!(
                knob.get().to_bits(),
                (units as f32 / 200.0).to_bits(),
                "{units}: значение должно быть ровно n/200"
            );
            assert_eq!(
                scale.units_of(knob),
                units,
                "{units}: число на экране совпадает с поставленным"
            );
        }
        // Значения пресета (литералы файла) совпадают с тем, что ставит палец, бит в бит.
        // Значение линейки пресета — `номер шага × шаг`, поэтому берём его так же, а не литералом f64.
        for (preset, units) in [
            (0.15_f32, 30),
            (-0.37, -74),
            (0.07, 14),
            (0.02, 4),
            (0.08, 16),
            (0.10, 20),
        ] {
            let knob = scale.knob_at(f64::from(units) * scale.step()).expect("ok");
            assert_eq!(
                knob.get().to_bits(),
                preset.to_bits(),
                "{preset}: литерал пресета = {units}/200"
            );
        }
    }

    #[test]
    fn non_finite_ruler_value_is_an_error_not_a_zero() {
        assert!(scale().knob_at(f64::NAN).is_err());
        assert!(scale().knob_at(f64::INFINITY).is_err());
    }
}
