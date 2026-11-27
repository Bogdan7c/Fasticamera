//! Высота столбика гистограммы: КАК число попаданий превращается в долю высоты картинки (сессия 14s).
//!
//! Какая шкала лучше смотрится на живых кадрах, владелец выбирает глазами по листам вариантов, поэтому шкала — НАСТРОЙКА
//! ([`HistogramHeight`] в [`crate::RenderSettings`]), а не зашитая формула. Шкала состоит из двух независимых частей:
//!
//! * кривая ([`HistogramCurve`]) — линейная, корень или `log2(1 + счёт)`: как сжимаются малые счёты рядом с большими;
//! * опора ([`HistogramReference`]) — счёт, которому соответствует ПОЛНАЯ высота: самый высокий столбик картинки
//!   или кратное среднего счёта на уровень.
//!
//! Счёт `n` — «сколько отсчётов легло на один уровень гистограммы»: плотность пикселя картинки, умноженная на число
//! пикселей картинки на уровень (перенос сетки в пиксели сохраняет число попаданий, см. [`crate::trace_resample`]).
//! Тогда ровная гистограмма из `c` отсчётов на уровень даёт `n = c` при любой ширине картинки, а опора «среднее ×
//! кратное» не зависит от размера картинки и числа уровней.
//!
//! Формула каждой кривой стоит и в шейдере `scope_render_histogram.comp` (тот же порядок операций f32): картинка GPU
//! сверяется с эталоном в допуске, а не бит в бит (там `sqrt`, `log2`, деление).

use serde::{Deserialize, Serialize};

/// Наибольшее кратное среднего счёта: дальше опора выше любого мыслимого столбика и шкала вырождается в «пусто».
pub const MAX_MEAN_MULTIPLE: f32 = 1_000.0;

/// Кривая высоты: как счёт `n` при опоре `r` превращается в долю высоты `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistogramCurve {
    /// `n / r`: честная пропорция, но один пик (чёрный или белый срез) прижимает остальное к нулю.
    Linear,
    /// `√(n / r)`: малые счёты заметнее, пики ниже.
    SquareRoot,
    /// `log2(1 + n) / log2(1 + r)`: как яркость следа waveform, видны даже тонкие хвосты (и шум сенсора).
    Log2,
}

impl HistogramCurve {
    /// Доля высоты для счёта `count` при опоре `reference` (оба — отсчётов на уровень): `0` при нулевом счёте или
    /// ненатуральной опоре, `1` при счёте не ниже опоры. ПОРЯДОК ОПЕРАЦИЙ f32 тот же, что в шейдере.
    pub fn fraction(self, count: f32, reference: f32) -> f32 {
        // Пустой уровень и «нет опоры» (кадр без отсчётов) — нет столбика; сравнение ложно и для NaN.
        if !(count > 0.0 && reference > 0.0) {
            return 0.0;
        }
        let fraction = match self {
            Self::Linear => count / reference,
            Self::SquareRoot => (count / reference).sqrt(),
            Self::Log2 => (1.0 + count).log2() / (1.0 + reference).log2(),
        };
        fraction.min(1.0)
    }

    /// Слово шейдера для кривой (малые целые, точно представимые в f32). `match` без `_`: новая кривая не соберётся
    /// без своего кода.
    pub const fn shader_code(self) -> f32 {
        match self {
            Self::Linear => 0.0,
            Self::SquareRoot => 1.0,
            Self::Log2 => 2.0,
        }
    }
}

/// Опора шкалы: какому счёту соответствует полная высота.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HistogramReference {
    /// Самый высокий столбик КАРТИНКИ достаёт до верха: форма читается всегда, но один пик гасит остальное.
    TallestBar,
    /// Полная высота — `multiple` × средний счёт на уровень (отсчётов кадра / уровней): пики выше опоры упираются в
    /// верх, зато тело гистограммы крупнее и от пика не зависит.
    MeanMultiple {
        /// Во сколько раз опора выше среднего счёта; `0 < multiple ≤ MAX_MEAN_MULTIPLE`.
        multiple: f32,
    },
}

/// Шкала высоты столбика: настройка показа гистограммы. Варианты — для листов выбора владельцем (решение 14s).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistogramHeight {
    /// Линейно до самого высокого столбика.
    LinearToPeak,
    /// Линейно до `multiple` × среднего.
    LinearToMean {
        /// Кратное среднего счёта.
        multiple: f32,
    },
    /// Корень до самого высокого столбика.
    SquareRootToPeak,
    /// Корень до `multiple` × среднего.
    SquareRootToMean {
        /// Кратное среднего счёта.
        multiple: f32,
    },
    /// `log2(1 + счёт)` до самого высокого столбика.
    Log2ToPeak,
    /// `log2(1 + счёт)` до `multiple` × среднего.
    Log2ToMean {
        /// Кратное среднего счёта.
        multiple: f32,
    },
}

impl HistogramHeight {
    /// Кривая шкалы.
    pub fn curve(self) -> HistogramCurve {
        match self {
            Self::LinearToPeak | Self::LinearToMean { .. } => HistogramCurve::Linear,
            Self::SquareRootToPeak | Self::SquareRootToMean { .. } => HistogramCurve::SquareRoot,
            Self::Log2ToPeak | Self::Log2ToMean { .. } => HistogramCurve::Log2,
        }
    }

    /// Опора шкалы.
    pub fn reference(self) -> HistogramReference {
        match self {
            Self::LinearToPeak | Self::SquareRootToPeak | Self::Log2ToPeak => HistogramReference::TallestBar,
            Self::LinearToMean { multiple } | Self::SquareRootToMean { multiple } | Self::Log2ToMean { multiple } => {
                HistogramReference::MeanMultiple { multiple }
            }
        }
    }

    /// Кратное среднего, если опора — среднее (для проверки значений).
    pub(crate) fn mean_multiple(self) -> Option<f32> {
        match self.reference() {
            HistogramReference::TallestBar => None,
            HistogramReference::MeanMultiple { multiple } => Some(multiple),
        }
    }

    /// Опора в отсчётах на уровень, ЕСЛИ она известна до рисования (среднее: отсчётов плоскости / уровней); `None`
    /// у опоры «самый высокий столбик» — её находит рисование по картинке.
    pub fn known_reference_count(self, samples_per_plane: u64, bins: u32) -> Option<f32> {
        match self.reference() {
            HistogramReference::TallestBar => None,
            HistogramReference::MeanMultiple { multiple } => {
                Some(multiple * (samples_per_plane as f32 / bins.max(1) as f32))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_curve_maps_zero_to_zero_the_reference_to_one_and_clamps_above() {
        for curve in [HistogramCurve::Linear, HistogramCurve::SquareRoot, HistogramCurve::Log2] {
            assert_eq!(curve.fraction(0.0, 100.0), 0.0, "{curve:?}");
            assert_eq!(curve.fraction(100.0, 100.0), 1.0, "{curve:?}");
            assert_eq!(curve.fraction(5_000.0, 100.0), 1.0, "{curve:?}");
            assert_eq!(curve.fraction(10.0, 0.0), 0.0, "{curve:?}: без опоры столбика нет");
            assert_eq!(curve.fraction(f32::NAN, 100.0), 0.0, "{curve:?}");
        }
    }

    #[test]
    fn at_a_quarter_of_the_reference_the_curves_give_their_own_heights() {
        assert_eq!(HistogramCurve::Linear.fraction(25.0, 100.0), 0.25);
        assert_eq!(HistogramCurve::SquareRoot.fraction(25.0, 100.0), 0.5);
        let log = HistogramCurve::Log2.fraction(25.0, 100.0);
        assert!((log - 26.0_f32.log2() / 101.0_f32.log2()).abs() < 1e-7, "{log}");
        assert!(log > 0.5, "лог-шкала поднимает малые счёты выше корня: {log}");
    }

    #[test]
    fn height_variants_split_into_curve_and_reference() {
        assert_eq!(HistogramHeight::LinearToPeak.curve(), HistogramCurve::Linear);
        assert_eq!(
            HistogramHeight::SquareRootToPeak.reference(),
            HistogramReference::TallestBar
        );
        let mean = HistogramHeight::Log2ToMean { multiple: 4.0 };
        assert_eq!(mean.curve(), HistogramCurve::Log2);
        assert_eq!(mean.reference(), HistogramReference::MeanMultiple { multiple: 4.0 });
        assert_eq!(mean.mean_multiple(), Some(4.0));
        assert_eq!(HistogramHeight::LinearToPeak.mean_multiple(), None);
    }

    #[test]
    fn the_mean_reference_is_known_in_advance_and_the_peak_reference_is_not() {
        // 512 000 отсчётов на 512 уровней — в среднем 1000 на уровень; кратное 8 даёт опору 8000.
        let mean = HistogramHeight::LinearToMean { multiple: 8.0 };
        assert_eq!(mean.known_reference_count(512_000, 512), Some(8_000.0));
        assert_eq!(HistogramHeight::LinearToPeak.known_reference_count(512_000, 512), None);
        assert_eq!(
            mean.known_reference_count(0, 512),
            Some(0.0),
            "кадр без отсчётов — опора 0, столбиков нет"
        );
    }

    #[test]
    fn heights_round_trip_through_json_in_snake_case() {
        let height = HistogramHeight::SquareRootToMean { multiple: 6.0 };
        let text = serde_json::to_string(&height).unwrap();
        assert_eq!(text, r#"{"square_root_to_mean":{"multiple":6.0}}"#);
        assert_eq!(serde_json::from_str::<HistogramHeight>(&text).unwrap(), height);
        assert_eq!(
            serde_json::to_string(&HistogramHeight::Log2ToPeak).unwrap(),
            r#""log2_to_peak""#
        );
    }
}
