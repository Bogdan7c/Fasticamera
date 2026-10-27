//! Арифметика меток времени GPU: маска значимых бит, тики → наносекунды, отрезки между метками.
//!
//! Общая для [`super::GpuTimestamps`] и диагностики порядка fence (`sync::diagnostics::timestamps`).

/// Маска значимых бит метки (`timestampValidBits` до 64).
pub(crate) fn valid_bits_mask(valid_bits: u32) -> u64 {
    if valid_bits >= 64 {
        u64::MAX
    } else {
        (1_u64 << valid_bits) - 1
    }
}

/// Разность `later − earlier` в тиках по модулю `2^valid_bits`: счётчик может быть короче 64 бит и
/// переходить через ноль. Корректно, пока интервал короче периода счётчика (на практике — минуты и
/// больше; кадр — миллисекунды).
pub(crate) fn elapsed_ticks(earlier: u64, later: u64, valid_bits: u32) -> u64 {
    later.wrapping_sub(earlier) & valid_bits_mask(valid_bits)
}

/// Тики → наносекунды (`timestampPeriod` — наносекунд на тик, дробное), с округлением.
pub(crate) fn ticks_to_nanoseconds(ticks: u64, period_ns: f32) -> u64 {
    (ticks as f64 * f64::from(period_ns)).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_covers_short_and_full_counters() {
        assert_eq!(valid_bits_mask(8), 0xFF);
        assert_eq!(valid_bits_mask(36), (1 << 36) - 1);
        assert_eq!(valid_bits_mask(64), u64::MAX);
    }

    #[test]
    fn elapsed_is_wrap_safe_for_short_counters() {
        // 8 бит: 250 → 10 — это 16 тиков вперёд, а не отрицательная разность.
        assert_eq!(elapsed_ticks(250, 10, 8), 16);
        assert_eq!(elapsed_ticks(100, 150, 64), 50);
        // Мусор в старших битах выше valid_bits не влияет.
        assert_eq!(elapsed_ticks(0xFF00_0000_0000_0005, 0x0000_0000_0000_0009, 32), 4);
    }

    #[test]
    fn nanoseconds_use_fractional_period() {
        // Adreno 740: период ~52.08 нс (19.2 МГц); RADV: 10 нс.
        assert_eq!(ticks_to_nanoseconds(96, 52.083_332), 5000);
        assert_eq!(ticks_to_nanoseconds(3, 10.0), 30);
        assert_eq!(ticks_to_nanoseconds(0, 52.0), 0);
    }
}
