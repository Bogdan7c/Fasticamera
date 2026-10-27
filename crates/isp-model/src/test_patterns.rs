//! Синтетические тестовые изображения и мозаики для проверки ISP (CPU-эталон и паритет GPU).
//!
//! Все генераторы детерминированы, возвращают линейный RGB камеры без шума и баланса белого
//! (серые паттерны имеют R = G = B). [`mosaic_from_rgb`] превращает такое изображение в мозаику
//! выбранной раскладки Bayer — «идеальный сенсор» без шума, по которому демозаик сравнивается с оригиналом.
//!
//! Генераторы возвращают `Result`: размер может быть пустым (для мозаики ещё и нечётным).

use camera_contract::geometry::PixelSize;

use crate::cfa::BayerPattern;
use crate::image::{ImageError, LinearCameraRgb, WhiteBalancedMosaic, mosaic_stage, space};

/// Сдвиг центра пикселя: пиксель (x, y) покрывает [x, x+1) × [y, y+1), его центр — (x+½, y+½).
const PIXEL_CENTRE_OFFSET: f32 = 0.5;
/// Серый уровень и амплитуда зонной пластинки: значения лежат в [0, 1] (`0.5 ± 0.5`).
const ZONE_PLATE_MEAN: f32 = 0.5;
const ZONE_PLATE_AMPLITUDE: f32 = 0.5;
/// Ширина линейного перехода края в пикселях: один пиксель ≈ фильтр «коробка» идеального сенсора,
/// поэтому край не имеет ступенчатой «лесенки», которой не бывает у реального сенсора.
const EDGE_TRANSITION_WIDTH_PIXELS: f32 = 1.0;

/// Серое изображение по функции яркости (x, y — центр пикселя в пикселях от левого верхнего угла).
fn gray_image(size: PixelSize, mut brightness: impl FnMut(f32, f32) -> f32) -> Result<LinearCameraRgb, ImageError> {
    let mut pixels = Vec::with_capacity(size.pixel_count() as usize);
    for y in 0..size.height {
        for x in 0..size.width {
            let level = brightness(x as f32 + PIXEL_CENTRE_OFFSET, y as f32 + PIXEL_CENTRE_OFFSET);
            pixels.push([level; 3]);
        }
    }
    LinearCameraRgb::from_pixels(size, pixels, space::CameraRgb)
}

/// Однородное изображение одного цвета (для проверок «плоское поле остаётся плоским»).
pub fn flat_color(size: PixelSize, color: [f32; 3]) -> Result<LinearCameraRgb, ImageError> {
    let pixels = vec![color; size.pixel_count() as usize];
    LinearCameraRgb::from_pixels(size, pixels, space::CameraRgb)
}

/// Зонная пластинка (серая): `0.5 + 0.5·cos(π·k·r²)`, центр в середине кадра.
///
/// Локальная частота равна производной фазы по радиусу: `f(r) = k·r` циклов на пиксель. Коэффициент
/// `k` подбирается так, чтобы в самом дальнем углу кадра частота была `max_frequency_cycles_per_pixel`
/// (предел Найквиста для сенсора — 0,5). Чем дальше от центра, тем выше частота и тем сильнее
/// демозаик показывает муар и ложные цвета.
pub fn zone_plate(size: PixelSize, max_frequency_cycles_per_pixel: f32) -> Result<LinearCameraRgb, ImageError> {
    let centre_x = size.width as f32 / 2.0;
    let centre_y = size.height as f32 / 2.0;
    let corner_radius = centre_x.hypot(centre_y);
    // k = f_max / r_max; для пустого размера делить не на что, но from_pixels всё равно вернёт ошибку.
    let chirp_rate = if corner_radius > 0.0 {
        max_frequency_cycles_per_pixel / corner_radius
    } else {
        0.0
    };
    gray_image(size, |x, y| {
        let radius_squared = (x - centre_x).powi(2) + (y - centre_y).powi(2);
        ZONE_PLATE_MEAN + ZONE_PLATE_AMPLITUDE * (std::f32::consts::PI * chirp_rate * radius_squared).cos()
    })
}

/// Наклонный край (серый): прямая через центр кадра, повёрнутая на `angle_degrees` от вертикали
/// (0° — вертикальный край, 45° — диагональный). Слева/сверху `dark`, справа/снизу `bright`; переход
/// шириной [`EDGE_TRANSITION_WIDTH_PIXELS`] по знаковому расстоянию до прямой.
pub fn slanted_edge(
    size: PixelSize,
    angle_degrees: f32,
    dark: f32,
    bright: f32,
) -> Result<LinearCameraRgb, ImageError> {
    let angle = angle_degrees.to_radians();
    // Нормаль к краю (единичная): при 0° смотрит вдоль +x.
    let (normal_x, normal_y) = (angle.cos(), -angle.sin());
    let centre_x = size.width as f32 / 2.0;
    let centre_y = size.height as f32 / 2.0;
    gray_image(size, |x, y| {
        let signed_distance = (x - centre_x) * normal_x + (y - centre_y) * normal_y;
        let bright_share = (signed_distance / EDGE_TRANSITION_WIDTH_PIXELS + PIXEL_CENTRE_OFFSET).clamp(0.0, 1.0);
        dark + (bright - dark) * bright_share
    })
}

/// Цвета полос «цветных полос» (EBU/SMPTE-порядок): белый, жёлтый, голубой, зелёный, пурпурный, красный, синий.
/// Все насыщенные: каждая вертикальная граница — резкий цветовой скачок, на котором демозаик даёт ложные цвета.
pub const COLOR_BAR_COLORS: [[f32; 3]; 7] = [
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [0.0, 1.0, 0.0],
    [1.0, 0.0, 1.0],
    [1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0],
];

/// Семь вертикальных цветных полос равной ширины ([`COLOR_BAR_COLORS`]) с яркостью `level` (0…1).
/// Границы полос резкие (без сглаживания) — это самый жёсткий случай для цветового демозаика.
pub fn color_bars(size: PixelSize, level: f32) -> Result<LinearCameraRgb, ImageError> {
    let bar_count = COLOR_BAR_COLORS.len() as u32;
    let mut pixels = Vec::with_capacity(size.pixel_count() as usize);
    for _row in 0..size.height {
        for x in 0..size.width {
            // Номер полосы: целочисленное деление x·7 / ширину (последняя полоса не вылезает).
            let bar = (u64::from(x) * u64::from(bar_count) / u64::from(size.width.max(1))) as usize;
            let color = COLOR_BAR_COLORS[bar.min(COLOR_BAR_COLORS.len() - 1)];
            pixels.push(color.map(|channel| channel * level));
        }
    }
    LinearCameraRgb::from_pixels(size, pixels, space::CameraRgb)
}

/// Идеальный сенсор: в каждом пикселе остаётся только тот канал, который пропускает фильтр `pattern`.
/// Размер изображения должен быть чётным (иначе квадрат 2×2 разрезан).
pub fn mosaic_from_rgb(image: &LinearCameraRgb, pattern: BayerPattern) -> Result<WhiteBalancedMosaic, ImageError> {
    let size = image.size();
    let mut samples = Vec::with_capacity(size.pixel_count() as usize);
    for y in 0..size.height {
        for x in 0..size.width {
            let channel = pattern.channel_at(x, y);
            samples.push(image.pixel(x, y)[channel.index()]);
        }
    }
    WhiteBalancedMosaic::from_samples(size, pattern, samples, mosaic_stage::WhiteBalanced)
}

#[cfg(test)]
#[path = "test_patterns_tests.rs"]
mod tests;
