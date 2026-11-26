//! Сколько отсчётов даёт показанная картинка прибору за кадр: ОДИН источник числа для эталона и GPU.
//!
//! От числа отсчётов зависит яркость следа (решение 266: полная яркость — с ДОЛИ отсчётов, а не с абсолютного
//! числа попаданий), поэтому эталон ([`crate::render`]) и GPU-сторона (`ColorScopes::record`) обязаны считать его
//! одинаково, иначе картинки разойдутся по яркости. Число считается тем же прореживанием, что и сам проход
//! накопления ([`SampleStride::samples_along`]): отсчёты по осям `x` и `y` независимы.

use crate::sampling::SampleStride;

/// Отсчёты кадра: `across` по горизонтали показанной картинки и `down` по вертикали.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleCount {
    across: u32,
    down: u32,
}

impl SampleCount {
    /// Отсчёты окна показанной картинки `width × height` при шаге `stride`.
    pub fn for_window(width: u32, height: u32, stride: SampleStride) -> Self {
        Self {
            across: stride.samples_along(width),
            down: stride.samples_along(height),
        }
    }

    /// Отсчётов по горизонтали (столбцов отсчётов).
    pub fn across(self) -> u32 {
        self.across
    }

    /// Отсчётов по вертикали (строк отсчётов).
    pub fn down(self) -> u32 {
        self.down
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
}
