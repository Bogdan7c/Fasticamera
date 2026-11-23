//! Шум после коррекции виньетирования — вход будущего шумодава (решение 170).
//!
//! Усиление в углах поднимает не только сигнал, но и шум, поэтому дисперсия шума в скорректированном
//! кадре зависит от места. Модель шума HAL (`SENSOR_NOISE_PROFILE`, DNG `NoiseProfile`) описывает шум
//! СЫРОГО сигнала: `N(x) = sqrt(S·x + O)`, то есть `σ²_raw(x) = S·x + O`, где `x` — сырой нормированный
//! сигнал, `S` и `O` — коэффициенты канала.
//!
//! # Вывод
//! Скорректированный отсчёт `c = g·x`, значит сырой сигнал `x = c/g`. Шум умножается на то же `g`:
//! ```text
//! σ²_corrected(c) = g² · σ²_raw(c/g) = g² · (S·c/g + O) = g·S·c + g²·O
//! ```
//! При `g = 1` формула возвращает исходную модель. Усиление нужно подставлять то, которое стадия
//! применила к именно этому отсчёту (с учётом силы коррекции).

use camera_contract::frame_passport::NoiseModelCoefficients;

/// Дисперсия шума отсчёта после коррекции виньетирования: `g·S·c + g²·O` (см. модуль).
///
/// * `profile_channel` — коэффициенты `S`, `O` канала CFA этого отсчёта;
/// * `corrected_signal` — скорректированное нормированное значение `c` (после уровней и усиления);
/// * `gain` — усиление, применённое стадией виньетирования к этому отсчёту.
pub fn shading_corrected_noise_variance(
    profile_channel: NoiseModelCoefficients,
    corrected_signal: f64,
    gain: f64,
) -> f64 {
    gain * profile_channel.scale * corrected_signal + gain * gain * profile_channel.offset
}

#[cfg(test)]
#[path = "shading_noise_tests.rs"]
mod tests;
