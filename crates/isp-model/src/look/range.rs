//! Сила ручки на краях шкалы: минус и плюс — отдельные числа.
//!
//! Экран показывает −200…+200 (число единиц — в конфиге экрана, решение 251), словарь хранит `[−1, 1]` ([`LookKnob`]), а что значит край — решает модель вида.
//! У многих ручек края физически разные: насыщенность в минус упирается в серый (множитель 0), а в плюс может
//! быть ×3. Один общий «диапазон» запирал плюс минусом, поэтому края разведены явным типом (сессия 14l).

use look_contract::LookKnob;
use serde::{Deserialize, Serialize};

/// Величина ручки на краях шкалы, в единицах самой ручки. Оба числа — модули (без знака).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnobRange {
    /// Модуль величины на краю −1,0 шкалы (−200 на экране).
    pub minus: f32,
    /// Модуль величины на краю +1,0 шкалы (+200 на экране).
    pub plus: f32,
}

impl KnobRange {
    /// Одинаковая сила в обе стороны.
    pub const fn symmetric(edge: f32) -> Self {
        Self {
            minus: edge,
            plus: edge,
        }
    }

    /// Величина ручки со знаком: ход ручки, умноженный на силу края своей стороны. Нейтраль — ровно `0.0`.
    pub fn amount(&self, knob: LookKnob) -> f32 {
        let travel = knob.get();
        if knob.is_neutral() {
            0.0
        } else if travel > 0.0 {
            travel * self.plus
        } else {
            travel * self.minus
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOPSIDED: KnobRange = KnobRange { minus: 1.0, plus: 2.0 };

    fn amount(travel: f32) -> f32 {
        LOPSIDED.amount(LookKnob::new(travel).unwrap())
    }

    #[test]
    fn each_side_uses_its_own_edge() {
        assert_eq!(amount(1.0), 2.0);
        assert_eq!(amount(0.5), 1.0);
        assert_eq!(amount(-1.0), -1.0);
        assert_eq!(amount(-0.5), -0.5);
    }

    #[test]
    fn neutral_is_exactly_positive_zero_even_for_negative_zero() {
        for zero in [0.0, -0.0] {
            assert_eq!(amount(zero).to_bits(), 0.0_f32.to_bits());
        }
    }

    #[test]
    fn unknown_field_is_rejected() {
        assert!(serde_json::from_str::<KnobRange>(r#"{"minus": 1.0, "plus": 2.0, "range": 3.0}"#).is_err());
        assert!(
            serde_json::from_str::<KnobRange>(r#"{"plus": 2.0}"#).is_err(),
            "оба края обязательны"
        );
    }
}
