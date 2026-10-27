//! Стадия 7: кривая передачи (OETF) выхода — sRGB по IEC 61966-2-1:1999, раздел 5.2.
//!
//! Display P3 использует ту же кривую (Apple Display P3 = основные цвета DCI-P3, белый D65,
//! передача sRGB), поэтому функция одна для обеих гамм.
//!
//! ```text
//! V = 12,92 · L                    при L ≤ 0,0031308
//! V = 1,055 · L^(1/2,4) − 0,055    иначе
//! ```

use crate::image::{DisplayEncodedImage, DisplayLinearImage, space};

/// Порог линейного участка на стороне света (IEC 61966-2-1).
const LINEAR_SEGMENT_LIMIT: f32 = 0.003_130_8;
/// Наклон линейного участка.
const LINEAR_SEGMENT_SLOPE: f32 = 12.92;
/// Показатель степенного участка: V ∝ L^(1/2,4).
const POWER_SEGMENT_EXPONENT: f32 = 2.4;
/// Масштаб и сдвиг степенного участка: 1,055 и 0,055.
const POWER_SEGMENT_SCALE: f32 = 1.055;
const POWER_SEGMENT_OFFSET: f32 = 0.055;
/// Порог линейного участка на стороне сигнала: 12,92 · 0,0031308 ≈ 0,04045 (обратная функция).
const ENCODED_LINEAR_LIMIT: f32 = 0.040_45;

/// Свет экрана 0…1 → сигнал 0…1. Вход обрезается в 0…1 (стадия тона уже свела свет в этот
/// диапазон); NaN → 0, как в тоне.
pub fn srgb_encode(linear: f32) -> f32 {
    if linear.is_nan() {
        return 0.0;
    }
    let linear = linear.clamp(0.0, 1.0);
    if linear <= LINEAR_SEGMENT_LIMIT {
        LINEAR_SEGMENT_SLOPE * linear
    } else {
        POWER_SEGMENT_SCALE * linear.powf(1.0 / POWER_SEGMENT_EXPONENT) - POWER_SEGMENT_OFFSET
    }
}

/// Обратная функция (EOTF): сигнал 0…1 → свет. Нужна метрикам и сверке с readback экрана.
pub fn srgb_decode(encoded: f32) -> f32 {
    let encoded = encoded.clamp(0.0, 1.0);
    if encoded <= ENCODED_LINEAR_LIMIT {
        encoded / LINEAR_SEGMENT_SLOPE
    } else {
        ((encoded + POWER_SEGMENT_OFFSET) / POWER_SEGMENT_SCALE).powf(POWER_SEGMENT_EXPONENT)
    }
}

/// Стадия 7: свет экрана → закодированный сигнал той же гаммы.
pub fn encode_display(image: &DisplayLinearImage) -> DisplayEncodedImage {
    let gamut = image.space().gamut;
    image.map_into(space::DisplayEncoded { gamut }, |pixel| pixel.map(srgb_encode))
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
