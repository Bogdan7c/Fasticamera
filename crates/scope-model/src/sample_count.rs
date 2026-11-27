//! Сколько отсчётов даёт показанная картинка прибору за кадр: ОДИН источник числа для эталона и GPU.
//!
//! От числа отсчётов зависят и форма сетки waveform/parade (столбец сетки = номер отсчёта вдоль взгляда, решение
//! 322), и яркость следа (решение 266: полная яркость — с плотности, а не с абсолютного числа попаданий). Поэтому
//! эталон ([`crate::render`], [`crate::accumulate`]) и GPU-сторона (`ColorScopes::record`) обязаны считать его
//! одинаково — оба зовут [`SampleCount::planned`]. Число считается тем же прореживанием, что и сам проход
//! накопления ([`SampleStride::samples_along`]): отсчёты по осям `x` и `y` независимы.

use scope_contract::ScopeScan;

use crate::sampling::{MAX_SAMPLE_STRIDE, SampleSteps, SampleStride};

/// Отсчёты кадра: `across` по горизонтали показанной картинки и `down` по вертикали, и шаги, которыми они взяты.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleCount {
    across: u32,
    down: u32,
    steps: SampleSteps,
}

impl SampleCount {
    /// Отсчёты окна показанной картинки `width × height` ровно при шагах `steps` (один шаг — на обе оси).
    pub fn for_window(width: u32, height: u32, steps: impl Into<SampleSteps>) -> Self {
        let steps = steps.into();
        Self {
            across: steps.across().samples_along(width),
            down: steps.down().samples_along(height),
            steps,
        }
    }

    /// План отсчётов кадра для прибора: шаг каждой оси не меньше `wanted`, но такой, чтобы отсчётов вдоль ЛЮБОЙ оси
    /// окна было не больше `max_along` — потолка столбцов сетки следа (`ScopeModelSettings::trace_max_columns`).
    ///
    /// Зачем: столбец сетки waveform/parade — это номер отсчёта вдоль взгляда, поэтому столбцов ровно столько, сколько
    /// отсчётов вдоль оси взгляда, а буфер счётчиков выделен под потолок заранее (внутри кадра ничего не выделяется).
    /// Окно шире потолка (на нынешних потоках телефона такого нет: длинная сторона ≤ 2048) берётся с бо́льшим шагом
    /// по ОБЕИМ осям, а не обрезается: прибор видит весь кадр, просто реже. Пол шага считается по длинной стороне
    /// окна, а не по оси взгляда, — поэтому он не меняется при повороте телефона (сами шаги осей при повороте
    /// меняются местами, если прибор просит разные вдоль и поперёк взгляда, решение 329). Если даже наибольший шаг
    /// не укладывает окно, берётся наибольший (сетка тогда отклонит лишние столбцы громко — ошибкой накопления).
    pub fn planned(width: u32, height: u32, wanted: SampleSteps, max_along: u32) -> Self {
        let floor = Self::smallest_stride_fitting(width.max(height), max_along);
        Self::for_window(width, height, wanted.at_least(floor))
    }

    /// Наименьший шаг, при котором вдоль стороны `longest` не больше `max_along` отсчётов (не больше наибольшего шага).
    fn smallest_stride_fitting(longest: u32, max_along: u32) -> SampleStride {
        let mut stride = SampleStride::EVERY_PIXEL;
        while stride.samples_along(longest) > max_along.max(1) && stride.get() < MAX_SAMPLE_STRIDE {
            // Шаг растёт на единицу: так он остаётся наименьшим подходящим (число шагов ≤ 64, это между кадрами
            // не дороже нескольких делений).
            stride = SampleStride::try_new(stride.get() + 1).unwrap_or(stride);
        }
        stride
    }

    /// Шаги прореживания, которыми взяты отсчёты (после плана — могут быть больше заданных в настройках).
    pub fn steps(self) -> SampleSteps {
        self.steps
    }

    /// Отсчётов по горизонтали показанной картинки (столбцов отсчётов).
    pub fn across(self) -> u32 {
        self.across
    }

    /// Отсчётов по вертикали показанной картинки (строк отсчётов).
    pub fn down(self) -> u32 {
        self.down
    }

    /// Отсчётов вдоль оси ВЗГЛЯДА «слева направо» при положении телефона `scan`: столько столбцов у сетки
    /// waveform/parade (решение 322).
    pub fn along_eye(self, scan: ScopeScan) -> u32 {
        if scan.runs_along_surface_rows() {
            self.down
        } else {
            self.across
        }
    }

    /// Отсчётов поперёк взгляда: столько отсчётов получает КАЖДЫЙ столбец сетки waveform/parade — поровну.
    pub fn across_eye(self, scan: ScopeScan) -> u32 {
        if scan.runs_along_surface_rows() {
            self.across
        } else {
            self.down
        }
    }

    /// Сколько пикселей кадра стоит за одним отсчётом: площадь блока шагов `шаг_x × шаг_y` (масса попадания при
    /// переносе в пиксели, решение 329).
    pub fn pixels_per_sample(self) -> u32 {
        self.steps.across().get() * self.steps.down().get()
    }

    /// Всего отсчётов за кадр.
    pub fn total(self) -> u64 {
        u64::from(self.across) * u64::from(self.down)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_is_the_product_of_per_axis_samples_of_the_stride() {
        // Шаг 2 на 10×7: x = 0,2,4,6,8 → 5 отсчётов? Смещение (2−1)/2 = 0, поэтому x = 0,2,4,6,8; y = 0,2,4,6.
        let count = SampleCount::for_window(10, 7, SampleStride::EVERY_SECOND_PIXEL);
        assert_eq!((count.across(), count.down(), count.total()), (5, 4, 20));
        let every_pixel = SampleCount::for_window(10, 7, SampleStride::EVERY_PIXEL);
        assert_eq!(every_pixel.total(), 70);
    }

    #[test]
    fn window_smaller_than_the_stride_offset_has_no_samples() {
        // Шаг 64: смещение 31; окно 20×20 отсчётов не даёт (счётчики остаются нулями — это не ошибка).
        let stride = SampleStride::try_new(64).unwrap();
        assert_eq!(SampleCount::for_window(20, 20, stride).total(), 0);
        // Одна ось пустая — пусто всё.
        assert_eq!(SampleCount::for_window(1000, 20, stride).total(), 0);
    }

    #[test]
    fn the_largest_shown_window_does_not_overflow() {
        let count = SampleCount::for_window(16_384, 16_384, SampleStride::EVERY_PIXEL);
        assert_eq!(count.total(), 16_384 * 16_384);
    }

    #[test]
    fn plan_keeps_the_wanted_stride_while_the_window_fits_under_the_column_ceiling() {
        // Раздел калибровки на телефоне: окно 917 × 2048, потолок 2048 — шаг 1 остаётся, отсчёты по всем пикселям.
        let plan = SampleCount::planned(917, 2048, SampleStride::EVERY_PIXEL.into(), 2048);
        assert_eq!(plan.steps(), SampleSteps::uniform(SampleStride::EVERY_PIXEL));
        assert_eq!((plan.across(), plan.down()), (917, 2048));
        assert_eq!(plan.along_eye(ScopeScan::Upright), 917);
        assert_eq!(plan.across_eye(ScopeScan::Upright), 2048);
        assert_eq!(plan.along_eye(ScopeScan::RotatedClockwise), 2048);
        assert_eq!(plan.across_eye(ScopeScan::RotatedCounterClockwise), 917);
    }

    #[test]
    fn plan_raises_the_stride_until_the_longest_side_fits_and_never_shrinks_the_wanted_one() {
        // 4000 px при потолке 2048: шаг 1 даёт 4000, шаг 2 — 2000 ≤ 2048.
        let plan = SampleCount::planned(4000, 3000, SampleStride::EVERY_PIXEL.into(), 2048);
        assert_eq!(plan.steps(), SampleSteps::uniform(SampleStride::EVERY_SECOND_PIXEL));
        assert!(plan.across() <= 2048 && plan.down() <= 2048);
        // Заданный шаг больше нужного — он и остаётся.
        let wanted = SampleStride::try_new(3).unwrap();
        assert_eq!(
            SampleCount::planned(100, 100, wanted.into(), 2048).steps(),
            SampleSteps::uniform(wanted)
        );
        // Потолок, которого не достичь даже шагом 64, даёт шаг 64, а не бесконечный цикл.
        assert_eq!(
            SampleCount::planned(16_384, 16_384, SampleStride::EVERY_PIXEL.into(), 1)
                .steps()
                .across()
                .get(),
            MAX_SAMPLE_STRIDE
        );
    }

    #[test]
    fn a_trace_plan_takes_every_sample_along_the_eye_and_every_second_across_it() {
        // Раздел калибровки: окно 917 × 2048, след просит 1 вдоль взгляда и 2 поперёк (решение 329).
        let (along, across) = (SampleStride::EVERY_PIXEL, SampleStride::EVERY_SECOND_PIXEL);
        let upright = SampleCount::planned(917, 2048, SampleSteps::for_eye(along, across, ScopeScan::Upright), 2048);
        // Столбцов сетки — все 917, каждому столбцу поровну 1024 отсчёта (вдвое меньше, чем при шаге 1).
        assert_eq!(
            (
                upright.along_eye(ScopeScan::Upright),
                upright.across_eye(ScopeScan::Upright)
            ),
            (917, 1024)
        );
        // Телефон боком: ось взгляда — вертикаль поверхности; столбцов — все 2048, поперёк — каждый второй из 917.
        let scan = ScopeScan::RotatedClockwise;
        let rotated = SampleCount::planned(917, 2048, SampleSteps::for_eye(along, across, scan), 2048);
        assert_eq!((rotated.along_eye(scan), rotated.across_eye(scan)), (2048, 459));
    }

    #[test]
    fn the_column_ceiling_raises_both_axes_to_the_same_floor_and_keeps_a_larger_wanted_step() {
        // 4000 px при потолке 2048: пол шага 2 по обеим осям; поперёк взгляда просили 3 — остаётся 3.
        let wanted = SampleSteps::new(SampleStride::EVERY_PIXEL, SampleStride::try_new(3).unwrap());
        let plan = SampleCount::planned(4000, 3000, wanted, 2048);
        assert_eq!((plan.steps().across().get(), plan.steps().down().get()), (2, 3));
        assert!(plan.across() <= 2048 && plan.down() <= 2048);
    }
}
