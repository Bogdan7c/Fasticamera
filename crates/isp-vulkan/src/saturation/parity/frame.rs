//! Синтетический RAW-кадр для линейки: отсчёты в числах (для GPU — упаковка в RAW10, для эталона — мозаика).

use camera_contract::geometry::{PixelRect, PixelSize};
use isp_model::cfa::BayerPattern;
use isp_model::image::{ImageError, RawMosaic};

/// Синтетический RAW-кадр (значения u16 в пикселях буфера).
#[derive(Debug, Clone)]
pub struct SyntheticRawFrame {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Раскладка цветных фильтров.
    pub pattern: BayerPattern,
    /// Отсчёты построчно.
    pub samples: Vec<u16>,
}

impl SyntheticRawFrame {
    /// Кадр из функции «значение RAW в пикселе (x, y) с цветом фильтра `channel` (0 R, 1 G, 2 B)».
    pub fn from_fn(width: u32, height: u32, pattern: BayerPattern, value: impl Fn(u32, u32, usize) -> u16) -> Self {
        let samples = (0..width * height)
            .map(|index| {
                let (x, y) = (index % width, index / width);
                value(x, y, pattern.channel_at(x, y).index())
            })
            .collect();
        Self {
            width,
            height,
            pattern,
            samples,
        }
    }

    /// Кадр из линейного света сенсора (0 — чёрный, 1 — белый), округление к ближайшему, обрезка до 1023 (RAW10).
    pub fn from_linear(
        width: u32,
        height: u32,
        pattern: BayerPattern,
        levels: ([f32; 4], f32),
        linear: impl Fn(u32, u32, usize) -> f64,
    ) -> Self {
        let (black, white) = levels;
        Self::from_fn(width, height, pattern, |x, y, channel| {
            let cell_black = f64::from(black[((y % 2) * 2 + (x % 2)) as usize]);
            let value = cell_black + linear(x, y, channel) * (f64::from(white) - cell_black);
            value.round().clamp(0.0, 1023.0) as u16
        })
    }

    /// Весь кадр как прямоугольник.
    pub fn full_rect(&self) -> PixelRect {
        PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(self.width, self.height),
        }
    }

    /// Мозаика эталона для прямоугольника (чётные края: раскладка CFA сохраняется).
    ///
    /// Прямоугольник обязан лежать внутри кадра (иначе индексация паникует: это ошибка вызывающего); раскладку CFA и
    /// размер мозаики проверяет сам `RawMosaic::new` и возвращает `ImageError`.
    pub fn mosaic(&self, rect: PixelRect) -> Result<RawMosaic, ImageError> {
        let mut samples = Vec::with_capacity(rect.size.pixel_count() as usize);
        for y in rect.top..rect.top + rect.size.height {
            for x in rect.left..rect.left + rect.size.width {
                samples.push(self.samples[(y * self.width + x) as usize]);
            }
        }
        RawMosaic::new(rect.size, self.pattern, samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_linear_maps_black_and_white_to_the_levels_and_clamps_to_ten_bits() {
        let frame = SyntheticRawFrame::from_linear(4, 2, BayerPattern::BGGR, ([64.0; 4], 1023.0), |x, _, _| match x {
            0 => 0.0,
            1 => 1.0,
            _ => 5.0,
        });
        assert_eq!(frame.samples[0], 64, "свет 0 = чёрный уровень");
        assert_eq!(frame.samples[1], 1023, "свет 1 = белый уровень");
        assert_eq!(frame.samples[2], 1023, "пересвет обрезан десятью битами");
    }

    #[test]
    fn mosaic_of_a_sub_rectangle_keeps_the_samples() {
        let frame = SyntheticRawFrame::from_fn(4, 4, BayerPattern::BGGR, |x, y, _| (y * 4 + x) as u16);
        let rect = PixelRect {
            left: 2,
            top: 2,
            size: PixelSize::new(2, 2),
        };
        let mosaic = frame.mosaic(rect).expect("мозаика собирается");
        let picked = [
            mosaic.sample(0, 0),
            mosaic.sample(1, 0),
            mosaic.sample(0, 1),
            mosaic.sample(1, 1),
        ];
        assert_eq!(picked, [10, 11, 14, 15]);
    }
}
