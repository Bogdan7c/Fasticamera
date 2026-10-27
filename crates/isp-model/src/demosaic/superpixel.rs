//! Быстрый демозаик для видоискателя: суперпиксель 2×2, половинное разрешение.
//!
//! Каждый квадрат Bayer 2×2 даёт один пиксель результата: R — отсчёт красной ячейки,
//! B — синей, G — среднее двух зелёных. Интерполяции нет, ложных цветов на краях нет,
//! цена — вдвое меньший размер.

use camera_contract::geometry::PixelSize;

use crate::cfa::{BayerPattern, CfaCell, RgbChannel};
use crate::image::{LinearCameraRgb, WhiteBalancedMosaic, space};

/// Пиксель результата для квадрата 2×2 с левым верхним углом (x0, y0).
fn superpixel_at(mosaic: &WhiteBalancedMosaic, x0: u32, y0: u32) -> [f32; 3] {
    let pattern: BayerPattern = mosaic.pattern();
    let mut color = [0.0_f32; 3];
    let mut green_sum = 0.0_f32;
    // Обходим четыре ячейки квадрата и раскладываем по цветам фильтра.
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (x, y) = (x0 + dx, y0 + dy);
        let sample = mosaic.sample(x, y);
        match pattern.channel_of_cell(CfaCell::of_pixel(x, y)) {
            RgbChannel::Green => green_sum += sample,
            channel => color[channel.index()] = sample,
        }
    }
    // В квадрате Bayer ровно две зелёные ячейки.
    color[RgbChannel::Green.index()] = green_sum / 2.0;
    color
}

/// Суперпиксельный демозаик: результат (W/2)×(H/2).
pub(super) fn demosaic_superpixel(mosaic: &WhiteBalancedMosaic) -> LinearCameraRgb {
    let size = mosaic.size();
    let half = PixelSize::new(size.width / 2, size.height / 2);
    let mut pixels = Vec::with_capacity(half.pixel_count() as usize);
    for y in 0..half.height {
        for x in 0..half.width {
            pixels.push(superpixel_at(mosaic, x * 2, y * 2));
        }
    }
    LinearCameraRgb::from_stage_output(half, pixels, space::CameraRgb)
}

#[cfg(test)]
#[path = "superpixel_tests.rs"]
mod tests;
