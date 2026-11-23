//! Общие помощники интеграционных тестов: синтетические картинки.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(dead_code)]

use scope_model::ShownCodes;

/// Картинка, каждый пиксель которой считает `pixel_at(x, y)`.
pub fn picture(width: u32, height: u32, pixel_at: impl Fn(u32, u32) -> [u16; 3]) -> ShownCodes {
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            pixels.push(pixel_at(x, y));
        }
    }
    ShownCodes::try_new(width, height, pixels).expect("синтетическая картинка корректна")
}

/// Картинка одного цвета.
pub fn flat(width: u32, height: u32, code: [u16; 3]) -> ShownCodes {
    picture(width, height, |_, _| code)
}

/// Код `round(amplitude · 1023)`.
pub fn code_of(amplitude: f64) -> u16 {
    (amplitude * 1023.0).round() as u16
}
