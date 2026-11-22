//! Правило перевода сигнала экрана в 8 бит — единственный источник истины для CPU-эталона и тестов;
//! шейдер `photo_output_rgba8.comp` повторяет его слово в слово.
//!
//! # Правило
//! `q = floor(clamp(e, 0, 1) × 255 + 0.5)`, где `e` — sRGB-кодированный сигнал экрана 0…1 (`display_encoded`
//! эталона `isp-model`, f32). Округление «половина вверх» (а не усечение): усечение смещало бы всю картинку
//! темнее на пол-градации (в среднем −0,5 LSB), что для снимка, который потом редактируют, — системная
//! ошибка. Зажим нужен, чтобы шум и выбросы тона не уходили за 0 и 255 (NaN эталон/шейдер превращают в 0 ещё
//! в OETF). Умножение и сложение — в f32 по отдельности, без слияния (FMA): CPU Rust его не делает, в
//! шейдере переменная `precise`.
//!
//! # Откуда расхождение GPU и CPU до 1 LSB
//! Матрицы, охват и тон считаются в шейдере бит-в-бит как на CPU, кроме `pow` (охват, тон, OETF): в Vulkan
//! его точность — «как у `exp2(y·log2 x)`», то есть несколько ULP f32 (около 1e-6 относительно). Сигнал `e`
//! поэтому может отличаться от эталона на ~1e-6, а `e × 255` — на ~3e-4 LSB; если `e × 255 + 0.5` лежит
//! ближе этого к целому, округление падает на другую сторону — разница ровно 1 LSB (не больше, потому что
//! значение отличается всего на тысячные доли шага).

/// 8-битное значение канала по правилу модуля.
pub fn quantize_display_encoded(encoded: f32) -> u8 {
    // `clamp` на NaN вернул бы NaN; `as u8` у NaN даёт 0 — но NaN сюда не доходит (OETF), страхуем явно.
    if encoded.is_nan() {
        return 0;
    }
    let scaled = encoded.clamp(0.0, 1.0) * 255.0;
    let rounded = (scaled + 0.5).floor();
    // rounded в 0..=255 по построению (clamp), приведение не теряет значение.
    rounded as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_clamp_and_midpoints_round_half_up() {
        assert_eq!(quantize_display_encoded(0.0), 0);
        assert_eq!(quantize_display_encoded(1.0), 255);
        assert_eq!(quantize_display_encoded(-0.3), 0, "ниже нуля зажимается");
        assert_eq!(quantize_display_encoded(1.7), 255, "выше единицы зажимается");
        assert_eq!(quantize_display_encoded(f32::NAN), 0);
        // 0,5 LSB ровно: 0.5/255 → «половина вверх» даёт 1, усечение дало бы 0.
        assert_eq!(quantize_display_encoded(0.5 / 255.0), 1);
        assert_eq!(quantize_display_encoded(0.49 / 255.0), 0);
        assert_eq!(quantize_display_encoded(127.5 / 255.0), 128);
        assert_eq!(quantize_display_encoded(254.4 / 255.0), 254);
        assert_eq!(quantize_display_encoded(254.6 / 255.0), 255);
    }

    #[test]
    fn every_level_survives_a_round_trip_through_its_exact_signal() {
        for level in 0..=255_u32 {
            assert_eq!(u32::from(quantize_display_encoded(level as f32 / 255.0)), level);
        }
    }
}
