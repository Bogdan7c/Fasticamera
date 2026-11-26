//! Опорное число яркости следа: сколько попаданий в ячейку дают ПОЛНУЮ яркость (решение 266).
//!
//! Раньше порог был абсолютным числом попаданий; теперь — ДОЛЯ отсчётов, поэтому картина не слепнет и не гаснет
//! при смене размера кадра, шага прореживания или числа столбцов:
//! * секции и наложение (waveform/parade): `порог = max(1, доля_столбца · отсчётов / столбцов)` — доля отсчётов
//!   ОДНОГО столбца (кадр делится на столбцы поровну);
//! * по положению (вектороскоп): `порог = max(1, доля_кадра · отсчётов)` — доля отсчётов кадра.
//!
//! Единственная функция [`TraceReference::for_paint`] отдаёт готовый f32-знаменатель `log2(1 + порог)`; её зовут и
//! эталон ([`crate::render`]), и GPU-сторона (поле `trace.x` блока показа), поэтому знаменатель у них один и тот же
//! бит в бит. Формула яркости `min(1, log2(1 + count) / знаменатель)` не меняется.
//!
//! Знаменатель НИКОГДА не берётся из суммы счётчиков: правила вектороскопа отбрасывают отсчёты вне сетки, и сумма
//! зависела бы от картинки (серый кадр и насыщенный давали бы разную яркость при том же числе отсчётов).

use crate::render_settings::RenderSettings;
use crate::sample_count::SampleCount;
use crate::trace_paint::TracePaint;

/// Яркость следа по счётчику относительно порога: `min(1, log2(1 + count) / log2(1 + порог))`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceReference {
    full_threshold: f32,
    log_denominator: f32,
}

impl TraceReference {
    /// Опорное число для способа рисования `paint`: `settings` — доли, `samples` — отсчёты кадра, `columns` —
    /// столбцов сетки счётчиков (нужно способам по столбцам; нуль столбцов трактуется как один).
    pub fn for_paint(paint: TracePaint, settings: &RenderSettings, samples: SampleCount, columns: u32) -> Self {
        let total = samples.total() as f32;
        let expected_hits = match paint {
            TracePaint::PlaneSections | TracePaint::PlanesOverlaid => {
                settings.trace_full_share_of_column.get() * (total / columns.max(1) as f32)
            }
            TracePaint::ColorByPosition => settings.trace_full_share_of_frame.get() * total,
        };
        Self::from_threshold(expected_hits.max(1.0))
    }

    /// Опорное число из готового порога (число попаданий полной яркости; не меньше 1).
    fn from_threshold(full_threshold: f32) -> Self {
        Self {
            full_threshold,
            log_denominator: (1.0 + full_threshold).log2(),
        }
    }

    /// Порог: число попаданий в ячейку, начиная с которого след полностью ярок (не меньше 1).
    pub fn full_threshold(self) -> f32 {
        self.full_threshold
    }

    /// Знаменатель лог-шкалы `log2(1 + порог)` — ровно тот f32, что уходит в шейдер (`trace.x`).
    pub fn log_denominator(self) -> f32 {
        self.log_denominator
    }

    /// Яркость следа ячейки со счётчиком `count`: 0 при нуле попаданий, 1 при пороге и выше.
    pub fn intensity(self, count: u32) -> f32 {
        self.intensity_scaled(count, 1.0)
    }

    /// Яркость ячейки столбца с масштабом счётчика `scale` ([`crate::ColumnCountScales`]). ПОРЯДОК ОПЕРАЦИЙ f32 тот
    /// же, что в шейдере `scope_render.comp`: `count · scale`, `1 + …`, `log2`, деление на знаменатель, `min` с 1.
    /// При `scale = 1.0` это прежняя формула бит в бит.
    pub fn intensity_scaled(self, count: u32, scale: f32) -> f32 {
        if count == 0 {
            return 0.0;
        }
        ((1.0 + count as f32 * scale).log2() / self.log_denominator).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_settings::TraceShare;
    use crate::sampling::SampleStride;

    fn samples(width: u32, height: u32) -> SampleCount {
        SampleCount::for_window(width, height, SampleStride::EVERY_SECOND_PIXEL)
    }

    #[test]
    fn column_paints_scale_with_samples_per_column_and_frame_paint_with_samples_of_the_frame() {
        let settings = RenderSettings::default();
        // 1000×500 при шаге 2 = 500 · 250 = 125 000 отсчётов; столбцов 250 → 500 отсчётов на столбец.
        let frame = samples(1000, 500);
        assert_eq!(frame.total(), 125_000);
        for paint in [TracePaint::PlaneSections, TracePaint::PlanesOverlaid] {
            let reference = TraceReference::for_paint(paint, &settings, frame, 250);
            assert_eq!(reference.full_threshold(), 0.02 * 500.0, "{paint:?}");
        }
        let position = TraceReference::for_paint(TracePaint::ColorByPosition, &settings, frame, 250);
        assert_eq!(position.full_threshold(), 0.0007 * 125_000.0);
        // Число столбцов вектороскопу безразлично.
        assert_eq!(
            position,
            TraceReference::for_paint(TracePaint::ColorByPosition, &settings, frame, 1)
        );
    }

    #[test]
    fn threshold_never_falls_below_one_hit_even_without_samples() {
        let settings = RenderSettings::default();
        let empty = SampleCount::for_window(1, 1, SampleStride::try_new(64).unwrap());
        assert_eq!(empty.total(), 0);
        for paint in TracePaint::ALL {
            let reference = TraceReference::for_paint(paint, &settings, empty, 512);
            assert_eq!(reference.full_threshold(), 1.0, "{paint:?}");
            assert_eq!(reference.log_denominator(), 2.0_f32.log2());
            assert_eq!(reference.intensity(1), 1.0, "одно попадание уже полная яркость");
        }
        // Нуль столбцов не делит на ноль.
        let degenerate = TraceReference::for_paint(TracePaint::PlaneSections, &settings, samples(100, 100), 0);
        assert!(degenerate.full_threshold().is_finite());
    }

    #[test]
    fn shares_from_settings_are_used_and_intensity_follows_the_log_scale() {
        let settings = RenderSettings {
            trace_full_share_of_column: TraceShare::try_new(0.5).unwrap(),
            ..RenderSettings::default()
        };
        // 40 отсчётов на столбец при доле 0,5 → полная яркость с 20 попаданий.
        let frame = samples(80, 40); // 40 · 20 = 800 отсчётов; столбцов 20
        let reference = TraceReference::for_paint(TracePaint::PlaneSections, &settings, frame, 20);
        assert_eq!(reference.full_threshold(), 20.0);
        assert_eq!(reference.log_denominator(), 21.0_f32.log2());
        assert_eq!(reference.intensity(0), 0.0);
        assert_eq!(reference.intensity(20), 1.0);
        assert_eq!(reference.intensity(5000), 1.0, "выше порога яркость не растёт");
        let expected = 6.0_f32.log2() / 21.0_f32.log2();
        assert!((reference.intensity(5) - expected).abs() < 1e-7);
        assert!(reference.intensity(5) < reference.intensity(10));
    }

    #[test]
    fn same_picture_keeps_its_brightness_when_the_frame_grows() {
        // Доля, а не абсолютное число: вдвое больший кадр требует вдвое больше попаданий на ту же яркость.
        let settings = RenderSettings::default();
        let small = TraceReference::for_paint(TracePaint::PlaneSections, &settings, samples(800, 400), 100);
        let large = TraceReference::for_paint(TracePaint::PlaneSections, &settings, samples(1600, 400), 100);
        assert_eq!(small.full_threshold(), 16.0);
        assert_eq!(large.full_threshold(), 32.0);
        assert_eq!(small.intensity(16), 1.0);
        assert_eq!(large.intensity(32), 1.0);
        assert!(
            large.intensity(16) < 1.0,
            "в большом кадре те же 16 попаданий — ещё не полная яркость"
        );
    }
}
