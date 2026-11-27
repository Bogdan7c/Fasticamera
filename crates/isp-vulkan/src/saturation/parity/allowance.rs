//! Допуски паритета: сколько ошибки математики GPU можно списать на формулу, а не на шейдер.

/// Допуск ошибки вычислений GPU сверх шага хранения (см. [`ErrorAllowance`]).
pub const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;
/// Полшага 10-битного выхода.
pub const HALF_LSB: f32 = 0.5 / 1023.0;
/// Шаг 10-битного выхода — «1 LSB» в отчётах паритета.
pub const ONE_LSB: f32 = 1.0 / 1023.0;

/// Сколько ошибки математики допускает случай. Действующий допуск один — [`ErrorAllowance::Standard`]; у трёх узких
/// групп случаев свой, с названной причиной: сама формула там раздувает последний разряд входа, и это видно по
/// эталону, а не по шейдеру. Все — много меньше половины ступени 10-битного экрана (4,9e-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAllowance {
    /// Допуск 14a.
    Standard,
    /// Прежняя основа, выход sRGB, цвет у края ручек 14l: рабочее пространство узкое, цвет уходит глубоко за охват, и
    /// канал у нуля набирается вычитанием крупных чисел; ≈ 1e-6 линейного света у кода 0,008 (линейный участок OETF,
    /// наклон 12,92) выходят как 1,0…1,6e-5. На прежних, слабых краях те же случаи давали 7e-7.
    LegacySrgbExtremeColour,
    /// Углы тона «контраст −, средние −, света −» разом: наклон кривой у белого ≈ 450, и 1–3 ULP разницы основы на
    /// CPU и GPU становятся 1e-5…1,3e-4.
    SteepToneAtWhite,
    /// Правки полос оттенка (сессия 14o, причина доказана опытом на ПК): вес полос «насколько цвет не серый»
    /// `c² / (c² + F²)` с `F = 0,01` (`look_stage.glsl`) у почти серого пикселя (цветность `c ≈ F`, кожа и пастель)
    /// раздувает разницу цветности в ≈ 60 раз. Сама разница — несколько ULP f32 между `pow`/`exp` основы на GPU и
    /// libm на CPU (≈ 1e-7 у AgX), а не ошибка ветки полос: с точным входом основы с CPU стадия вида на GPU совпала с
    /// эталоном до 1e-8. Худший пиксель на нейтрали D65: ≈ 2,5e-5 разницы до округления хранения (на fp16 видно
    /// 1,35e-5 сверх шага, на rgb10a2 — 6,1e-6, его прячет округление к ближайшему). Допуск 3e-5 закрывает это с
    /// запасом и всё равно в 16 раз ниже половины ступени экрана.
    NearNeutralBandsAmplification,
}

impl ErrorAllowance {
    /// Допуск ошибки математики сверх шага хранения.
    pub fn math_excess(self) -> f32 {
        match self {
            Self::Standard => MATH_EXCESS_TOLERANCE,
            Self::LegacySrgbExtremeColour => 2.0e-5,
            Self::SteepToneAtWhite => 2.0e-4,
            Self::NearNeutralBandsAmplification => 3.0e-5,
        }
    }

    /// Стабильное короткое имя допуска для отчётов (его читают разборщики в `scripts/`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::LegacySrgbExtremeColour => "legacy_srgb_extreme_colour",
            Self::SteepToneAtWhite => "steep_tone_at_white",
            Self::NearNeutralBandsAmplification => "near_neutral_bands_amplification",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_allowances_are_looser_than_standard_but_below_half_a_display_step() {
        let standard = ErrorAllowance::Standard.math_excess();
        assert_eq!(standard, MATH_EXCESS_TOLERANCE);
        for allowance in [
            ErrorAllowance::LegacySrgbExtremeColour,
            ErrorAllowance::SteepToneAtWhite,
            ErrorAllowance::NearNeutralBandsAmplification,
        ] {
            assert!(allowance.math_excess() > standard, "{allowance:?}");
            assert!(allowance.math_excess() < HALF_LSB, "{allowance:?}");
        }
    }

    #[test]
    fn allowance_names_are_distinct_and_stable() {
        let names = [
            ErrorAllowance::Standard.name(),
            ErrorAllowance::LegacySrgbExtremeColour.name(),
            ErrorAllowance::SteepToneAtWhite.name(),
            ErrorAllowance::NearNeutralBandsAmplification.name(),
        ];
        assert_eq!(
            names,
            [
                "standard",
                "legacy_srgb_extreme_colour",
                "steep_tone_at_white",
                "near_neutral_bands_amplification"
            ]
        );
    }
}
