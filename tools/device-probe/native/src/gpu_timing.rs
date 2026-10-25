//! Перевод GPU-timestamp'ов в микросекунды (чистая арифметика, тестируется на host).

/// Разница двух timestamp'ов в микросекундах с учётом ширины счётчика (`valid_bits`).
pub fn ticks_to_microseconds(start: u64, end: u64, valid_bits: u32, period_ns: f32) -> f64 {
    let mask = if valid_bits >= 64 {
        u64::MAX
    } else {
        (1_u64 << valid_bits) - 1
    };
    let elapsed_ticks = end.wrapping_sub(start) & mask;
    elapsed_ticks as f64 * f64::from(period_ns) / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_convert_with_period() {
        // 1000 тиков по 50 нс = 50 мкс
        let microseconds = ticks_to_microseconds(100, 1100, 64, 50.0);
        assert!((microseconds - 50.0).abs() < 1e-9);
    }

    #[test]
    fn narrow_counter_wraparound_is_handled() {
        // 48-битный счётчик: end < start означает переполнение.
        let mask = (1_u64 << 48) - 1;
        let microseconds = ticks_to_microseconds(mask - 9, 10, 48, 1000.0);
        assert!((microseconds - 20.0).abs() < 1e-9); // 20 тиков × 1000 нс = 20 мкс
    }
}
