//! Основные цвета RGB waveform: каким цветом рисуются три плоскости, наложенные друг на друга (решение 265).
//!
//! След каждого канала красится «своим» основным цветом; в пикселе, где следы совпали, цвета складываются, и
//! СЕРЫЙ кадр (три канала равны) обязан дать БЕЛЫЙ след. Для этого основные цвета палитры по каждому каналу
//! экрана в сумме дают ровно 1: `P_R[k] + P_G[k] + P_B[k] = 1` для `k = R, G, B`.
//!
//! * «чистая» палитра — настоящие красный, зелёный и синий: сумма единичная сама собой;
//! * «смягчённая» (по умолчанию) — те же оттенки, подсвеченные для читаемости на тёмном (в каждом основном цвете
//!   «чужие» каналы приподняты). Сырые оттенки приподняты произвольно, поэтому сумма по каналу сначала не равна
//!   единице; нормировка делит каждый канал на его сумму по трём цветам — и белый след снова точно белый.

use serde::{Deserialize, Serialize};

/// Сырые оттенки «смягчённой» палитры ДО нормировки (строки — каналы R, G, B; столбцы — R, G, B экрана).
const SOFTENED_RAW: [[f64; 3]; 3] = [[1.0, 0.28, 0.28], [0.30, 1.0, 0.35], [0.20, 0.35, 1.0]];

/// Какие основные цвета использует RGB waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RgbWaveformPalette {
    /// Подсвеченные основные, нормированные так, что сумма по каждому каналу ровно 1 (по умолчанию).
    #[default]
    Softened,
    /// Чистые красный, зелёный, синий.
    Pure,
}

impl RgbWaveformPalette {
    /// Основные цвета плоскостей R, G, B (каждый — цвет экрана `[r, g, b]`, каналы 0..=1).
    pub fn primaries(self) -> [[f32; 3]; 3] {
        match self {
            Self::Pure => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            Self::Softened => {
                // Сумма по трём цветам для каждого канала экрана; на неё делится каждый основной цвет.
                let channel_sums: [f64; 3] =
                    std::array::from_fn(|channel| SOFTENED_RAW.iter().map(|raw| raw[channel]).sum());
                SOFTENED_RAW.map(|raw| std::array::from_fn(|channel| (raw[channel] / channel_sums[channel]) as f32))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softened_primaries_sum_to_white_per_channel_and_match_the_decision_numbers() {
        let primaries = RgbWaveformPalette::Softened.primaries();
        for channel in 0..3 {
            let sum: f32 = primaries.iter().map(|primary| primary[channel]).sum();
            assert!((sum - 1.0).abs() < 1e-6, "канал {channel}: сумма {sum}");
        }
        let expected = [
            [0.66667, 0.17178, 0.17178],
            [0.2, 0.61350, 0.21472],
            [0.13333, 0.21472, 0.61350],
        ];
        for (primary, wanted) in primaries.iter().zip(expected) {
            for channel in 0..3 {
                assert!(
                    (primary[channel] - wanted[channel]).abs() < 1e-4,
                    "{primary:?} против {wanted:?}"
                );
            }
        }
    }

    #[test]
    fn pure_primaries_are_the_identity_and_the_default_is_softened() {
        assert_eq!(
            RgbWaveformPalette::Pure.primaries(),
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(RgbWaveformPalette::default(), RgbWaveformPalette::Softened);
        assert_ne!(
            RgbWaveformPalette::Pure.primaries(),
            RgbWaveformPalette::Softened.primaries()
        );
    }

    #[test]
    fn palette_is_read_from_snake_case_json() {
        let pure: RgbWaveformPalette = serde_json::from_str("\"pure\"").unwrap();
        assert_eq!(pure, RgbWaveformPalette::Pure);
        assert!(serde_json::from_str::<RgbWaveformPalette>("\"Pure\"").is_err());
    }
}
