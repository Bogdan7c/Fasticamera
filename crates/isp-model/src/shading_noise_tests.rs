use camera_contract::frame_passport::NoiseModelCoefficients;

use super::shading_corrected_noise_variance;

/// Детерминированный генератор равномерных чисел (xorshift64*): тест не зависит от внешних crate-ов
/// и всегда даёт одну и ту же выборку.
struct Uniform(u64);

impl Uniform {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        (bits as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Стандартное нормальное число (Бокс — Мюллер).
    fn gaussian(&mut self) -> f64 {
        let (u1, u2) = (self.next(), self.next());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Измеряет дисперсию `g · raw` по выборке, где `raw = x + N(0, S·x + O)`.
fn measured_corrected_variance(profile: NoiseModelCoefficients, raw_signal: f64, gain: f64) -> (f64, f64) {
    let mut rng = Uniform(0x9E37_79B9_7F4A_7C15);
    let sigma = (profile.scale * raw_signal + profile.offset).sqrt();
    let samples = 400_000;
    let corrected: Vec<f64> = (0..samples)
        .map(|_| gain * (raw_signal + sigma * rng.gaussian()))
        .collect();
    let mean = corrected.iter().sum::<f64>() / samples as f64;
    let variance = corrected.iter().map(|value| (value - mean).powi(2)).sum::<f64>() / (samples - 1) as f64;
    (mean, variance)
}

#[test]
fn formula_matches_the_measured_variance_of_amplified_raw_noise() {
    let profile = NoiseModelCoefficients {
        scale: 0.004,
        offset: 0.000_02,
    };
    for (raw_signal, gain) in [(0.05, 1.0), (0.05, 2.5), (0.3, 4.3), (0.6, 1.6)] {
        let (mean, measured) = measured_corrected_variance(profile, raw_signal, gain);
        // Формула получает скорректированный сигнал c = g·x — среднее выборки.
        let predicted = shading_corrected_noise_variance(profile, mean, gain);
        let relative_error = (measured - predicted).abs() / predicted;
        assert!(
            relative_error < 0.02,
            "x = {raw_signal}, g = {gain}: измерено {measured:e}, формула {predicted:e}, ошибка {relative_error:.4}"
        );
    }
}

#[test]
fn a_naive_raw_model_would_underestimate_the_noise_in_the_corners() {
    let profile = NoiseModelCoefficients {
        scale: 0.004,
        offset: 0.000_02,
    };
    // Ошибочная модель «шум не зависит от усиления» занижала бы дисперсию на углу с g = 4.
    let corrected_signal = 0.2;
    let with_gain = shading_corrected_noise_variance(profile, corrected_signal, 4.0);
    let raw_model = shading_corrected_noise_variance(profile, corrected_signal, 1.0);
    assert!(with_gain > 3.9 * raw_model, "{with_gain:e} против {raw_model:e}");
}

#[test]
fn unit_gain_returns_the_original_noise_model() {
    let profile = NoiseModelCoefficients {
        scale: 0.002,
        offset: 0.00001,
    };
    assert_eq!(
        shading_corrected_noise_variance(profile, 0.5, 1.0),
        0.002 * 0.5 + 0.00001
    );
}
