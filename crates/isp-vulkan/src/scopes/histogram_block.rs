//! Хвост блока показа для гистограммы (сессия 14s): `vec4 histogram` в `scope_render_parameters.glsl`.
//!
//! * `x` — опора шкалы высоты в отсчётах на уровень, если она известна до рисования (кратное среднего), либо
//!   [`TALLEST_BAR_REFERENCE`] — «самый высокий столбик картинки»: его находит сам шейдер показа;
//! * `y` — код кривой высоты ([`scope_model::HistogramCurve::shader_code`]);
//! * `z` — непрозрачность заливки, `w` — толщина линии верха в пикселях (решение 337).
//!
//! Следом `vec4 histogram_style`: `x` — множитель цвета заливки, `y` — зазор между полосами PARADE долей высоты картинки,
//! `z`, `w` — 0.
//!
//! У способов, которые столбиков не рисуют, вектор нулевой и шейдером не читается.

use scope_model::{HistogramHeight, HistogramLook, HistogramReference, SampleCount, TracePaint};

/// Значение `x`, которым опора «самый высокий столбик» отличается от любого настоящего счёта (счёт не бывает
/// отрицательным). Шейдер сравнивает `x >= 0`.
pub(super) const TALLEST_BAR_REFERENCE: f32 = -1.0;

/// Слова `vec4 histogram` и `vec4 histogram_style` для способа `paint` при шкале `height`, виде `look`, отсчётах кадра
/// `samples` и `bins` уровнях.
pub(super) fn histogram_words(
    paint: TracePaint,
    height: HistogramHeight,
    look: HistogramLook,
    samples: SampleCount,
    bins: u32,
) -> [u32; 8] {
    if !paint.draws_bars() {
        return [0; 8];
    }
    let reference = match height.reference() {
        HistogramReference::TallestBar => TALLEST_BAR_REFERENCE,
        // Среднее известно по числу отсчётов кадра: тот же вызов, что у эталона (`HistogramBars::new`).
        HistogramReference::MeanMultiple { .. } => height
            .known_reference_count(samples.total(), bins)
            .unwrap_or(TALLEST_BAR_REFERENCE),
    };
    [
        reference.to_bits(),
        height.curve().shader_code().to_bits(),
        look.fill_opacity.to_bits(),
        look.edge_width_px.to_bits(),
        look.fill_tint.to_bits(),
        look.strip_gap.to_bits(),
        0,
        0,
    ]
}

#[cfg(test)]
mod tests {
    use scope_model::{RenderSettings, SampleStride, TracePaint};

    use super::*;

    fn samples() -> SampleCount {
        // 1000 × 512 отсчётов.
        SampleCount::for_window(1000, 512, SampleStride::EVERY_PIXEL)
    }

    fn look() -> HistogramLook {
        RenderSettings::default().histogram_look()
    }

    fn floats(words: [u32; 8]) -> [f32; 8] {
        words.map(f32::from_bits)
    }

    #[test]
    fn the_tallest_bar_reference_is_marked_negative_and_the_curve_goes_by_its_code() {
        let words = floats(histogram_words(
            TracePaint::HistogramOverlaid,
            HistogramHeight::SquareRootToPeak,
            look(),
            samples(),
            512,
        ));
        assert_eq!(words[..2], [TALLEST_BAR_REFERENCE, 1.0]);
        let words = floats(histogram_words(
            TracePaint::HistogramSections,
            HistogramHeight::Log2ToPeak,
            look(),
            samples(),
            512,
        ));
        assert_eq!(words[..2], [TALLEST_BAR_REFERENCE, 2.0]);
    }

    #[test]
    fn the_look_settings_ride_along_with_the_height_scale() {
        let look = HistogramLook {
            fill_opacity: 0.4,
            fill_tint: 0.6,
            edge_width_px: 2.5,
            strip_gap: 0.03,
        };
        let words = floats(histogram_words(
            TracePaint::HistogramSections,
            HistogramHeight::LinearToPeak,
            look,
            samples(),
            512,
        ));
        assert_eq!(words[2..], [0.4, 2.5, 0.6, 0.03, 0.0, 0.0]);
    }

    #[test]
    fn the_mean_reference_is_the_multiple_of_the_samples_per_bin() {
        // 512 000 отсчётов на 512 уровней — в среднем 1000 на уровень; кратное 3 даёт опору 3000.
        let words = floats(histogram_words(
            TracePaint::HistogramSections,
            HistogramHeight::LinearToMean { multiple: 3.0 },
            look(),
            samples(),
            512,
        ));
        assert_eq!(words[..2], [3000.0, 0.0]);
        let empty = SampleCount::for_window(1, 1, SampleStride::try_new(64).unwrap());
        let words = floats(histogram_words(
            TracePaint::HistogramSections,
            HistogramHeight::LinearToMean { multiple: 3.0 },
            look(),
            empty,
            512,
        ));
        assert_eq!(words[0], 0.0, "кадр без отсчётов: опора 0, а не «самый высокий»");
    }

    #[test]
    fn paints_without_bars_get_a_zero_vector_whatever_the_height_setting() {
        for paint in [
            TracePaint::PlaneSections,
            TracePaint::PlanesOverlaid,
            TracePaint::ColorByPosition,
        ] {
            assert_eq!(
                histogram_words(
                    paint,
                    HistogramHeight::LinearToMean { multiple: 3.0 },
                    look(),
                    samples(),
                    512
                ),
                [0; 8],
                "{paint:?}"
            );
        }
    }
}
