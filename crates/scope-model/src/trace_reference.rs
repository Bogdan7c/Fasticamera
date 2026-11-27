//! Опорное число яркости следа: сколько попаданий НА ПИКСЕЛЬ картинки дают полную яркость (решения 266, 322).
//!
//! Счёт пикселя после переноса сетки в картинку ([`crate::trace_resample`]) — это «сколько попаданий легло на этот
//! пиксель» (дробное число: ядро сохраняет попадания). Порог полной яркости — плотность [`FullDensity`], во сколько
//! раз гуще равномерной: равномерная — все отсчёты кадра, разложенные по пикселям картинки поровну:
//! * наложение (RGB waveform): `отсчётов / (ширина · высота)` — каждый отсчёт кладёт по попаданию в каждую плоскость,
//!   плоскости рисуются в одни и те же пиксели;
//! * секции (parade): `отсчётов · плоскостей / (ширина · высота)` — плоскость занимает свою долю ширины;
//! * по положению (вектороскоп): `отсчётов / (ширина · высота)`.
//!
//! Порог = `max(1, плотность · равномерная)` попаданий на пиксель. Яркость `min(1, log2(1 + счёт) / log2(1 + порог))`
//! (лог-шкала решения 266 не меняется). Порог не зависит ни от размера кадра и шага, ни от размера картинки прибора:
//! вдвое больший кадр вдвое поднимает и счёт, и порог.
//!
//! Единственная функция [`TraceReference::for_paint`] отдаёт готовый f32-знаменатель `log2(1 + порог)`; её зовут и
//! эталон ([`crate::render`]), и GPU-сторона (поле `trace.x` блока показа), поэтому знаменатель у них один бит в бит.
//!
//! Порог НИКОГДА не берётся из суммы счётчиков: правила вектороскопа отбрасывают отсчёты вне сетки, и сумма
//! зависела бы от картинки (серый кадр и насыщенный давали бы разную яркость при том же числе отсчётов).

use scope_contract::ScopeImageSize;

use crate::render_settings::RenderSettings;
use crate::sample_count::SampleCount;
use crate::trace_paint::TracePaint;

/// Яркость следа по счёту пикселя относительно порога: `min(1, log2(1 + счёт) / log2(1 + порог))`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceReference {
    full_threshold: f32,
    log_denominator: f32,
    /// Пикселей кадра за одним отсчётом (масса попадания при переносе, решение 329).
    pixels_per_hit: u32,
}

impl TraceReference {
    /// Опорное число для способа рисования `paint`: `settings` — плотности полной яркости, `samples` — отсчёты кадра,
    /// `picture` — размер картинки прибора, `planes` — плоскостей сетки (нужно секциям).
    pub fn for_paint(
        paint: TracePaint,
        settings: &RenderSettings,
        samples: SampleCount,
        picture: ScopeImageSize,
        planes: u32,
    ) -> Self {
        // Порог — в «пикселях кадра», как и счёт после переноса (каждое попадание несёт массу своего блока): так он
        // не зависит от прореживания.
        let pixels_per_hit = samples.pixels_per_sample();
        let total = samples.total() as f32 * pixels_per_hit as f32;
        let picture_pixels = picture.width() as f32 * picture.height() as f32;
        let (density, pixels_per_plane) = match paint {
            TracePaint::PlanesOverlaid => (settings.column_trace_full_density, picture_pixels),
            TracePaint::PlaneSections => (
                settings.column_trace_full_density,
                picture_pixels / planes.max(1) as f32,
            ),
            TracePaint::ColorByPosition => (settings.position_trace_full_density, picture_pixels),
        };
        let uniform_hits_per_pixel = total / pixels_per_plane;
        Self::from_threshold((density.get() * uniform_hits_per_pixel).max(1.0), pixels_per_hit)
    }

    /// Опорное число из готового порога (попаданий на пиксель полной яркости; не меньше 1).
    fn from_threshold(full_threshold: f32, pixels_per_hit: u32) -> Self {
        Self {
            full_threshold,
            log_denominator: (1.0 + full_threshold).log2(),
            pixels_per_hit,
        }
    }

    /// Пикселей кадра за одним отсчётом: масса попадания, которую несёт ядро вертикали при переносе
    /// ([`crate::AxisResampling::carrying_pixels_per_hit`]); GPU-сторона кладёт её в то же ядро.
    pub fn pixels_per_hit(self) -> u32 {
        self.pixels_per_hit
    }

    /// Порог: попаданий на пиксель, начиная с которого след полностью ярок (не меньше 1).
    pub fn full_threshold(self) -> f32 {
        self.full_threshold
    }

    /// Знаменатель лог-шкалы `log2(1 + порог)` — ровно тот f32, что уходит в шейдер (`trace.x`).
    pub fn log_denominator(self) -> f32 {
        self.log_denominator
    }

    /// Яркость пикселя со счётом `hits` (попаданий на пиксель, дробное после переноса): 0 при нуле и ниже, 1 при
    /// пороге и выше. ПОРЯДОК ОПЕРАЦИЙ f32 тот же, что в шейдере `scope_render.comp`: `1 + счёт`, `log2`, деление
    /// на знаменатель, `min` с 1.
    pub fn intensity(self, hits: f32) -> f32 {
        if hits <= 0.0 {
            return 0.0;
        }
        ((1.0 + hits).log2() / self.log_denominator).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_settings::FullDensity;
    use crate::sampling::SampleStride;

    fn samples(width: u32, height: u32) -> SampleCount {
        SampleCount::for_window(width, height, SampleStride::EVERY_PIXEL)
    }

    fn picture(width: u32, height: u32) -> ScopeImageSize {
        ScopeImageSize::try_new(width, height).unwrap()
    }

    #[test]
    fn threshold_is_the_density_times_the_uniform_hits_per_pixel_of_each_paint() {
        let settings = RenderSettings::default();
        // 1000 × 500 = 500 000 отсчётов на картинку 1000 × 250 = 250 000 px → равномерно 2 попадания на пиксель.
        let frame = samples(1000, 500);
        let size = picture(1000, 250);
        let overlaid = TraceReference::for_paint(TracePaint::PlanesOverlaid, &settings, frame, size, 3);
        assert_eq!(overlaid.full_threshold(), 5.2 * 2.0);
        // Секции: плоскость на трети ширины — равномерно втрое гуще.
        let sections = TraceReference::for_paint(TracePaint::PlaneSections, &settings, frame, size, 3);
        assert!((sections.full_threshold() - 5.2 * 6.0).abs() < 1e-4);
        let position = TraceReference::for_paint(TracePaint::ColorByPosition, &settings, frame, size, 1);
        assert_eq!(position.full_threshold(), 46.0 * 2.0);
    }

    #[test]
    fn threshold_never_falls_below_one_hit_even_without_samples() {
        let settings = RenderSettings::default();
        let empty = SampleCount::for_window(1, 1, SampleStride::try_new(64).unwrap());
        assert_eq!(empty.total(), 0);
        for paint in TracePaint::ALL {
            let reference = TraceReference::for_paint(paint, &settings, empty, picture(100, 100), 0);
            assert_eq!(reference.full_threshold(), 1.0, "{paint:?}");
            assert_eq!(reference.log_denominator(), 2.0_f32.log2());
            assert_eq!(reference.intensity(1.0), 1.0, "одно попадание уже полная яркость");
            assert_eq!(reference.intensity(0.0), 0.0);
        }
    }

    #[test]
    fn density_from_settings_is_used_and_intensity_follows_the_log_scale() {
        let settings = RenderSettings {
            column_trace_full_density: FullDensity::try_new(10.0).unwrap(),
            ..RenderSettings::default()
        };
        // 200 × 100 отсчётов на 200 × 100 px: равномерно 1 на пиксель, порог 10.
        let reference = TraceReference::for_paint(
            TracePaint::PlanesOverlaid,
            &settings,
            samples(200, 100),
            picture(200, 100),
            3,
        );
        assert_eq!(reference.full_threshold(), 10.0);
        assert_eq!(reference.intensity(10.0), 1.0);
        assert_eq!(reference.intensity(5000.0), 1.0, "выше порога яркость не растёт");
        let expected = 6.0_f32.log2() / 11.0_f32.log2();
        assert!((reference.intensity(5.0) - expected).abs() < 1e-7);
        assert!(reference.intensity(0.25) < reference.intensity(0.5));
    }

    #[test]
    fn brightness_does_not_depend_on_the_frame_or_the_picture_size() {
        // Вдвое больший кадр — вдвое больше попаданий на пиксель и вдвое выше порог; вдвое больше картинка — наоборот.
        let settings = RenderSettings::default();
        let base = TraceReference::for_paint(
            TracePaint::PlanesOverlaid,
            &settings,
            samples(800, 400),
            picture(400, 400),
            3,
        );
        let big_frame = TraceReference::for_paint(
            TracePaint::PlanesOverlaid,
            &settings,
            samples(1600, 400),
            picture(400, 400),
            3,
        );
        let big_picture = TraceReference::for_paint(
            TracePaint::PlanesOverlaid,
            &settings,
            samples(800, 400),
            picture(800, 400),
            3,
        );
        assert_eq!(big_frame.full_threshold(), 2.0 * base.full_threshold());
        assert_eq!(big_picture.full_threshold(), 0.5 * base.full_threshold());
    }
}
