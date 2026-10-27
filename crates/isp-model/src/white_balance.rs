//! Стадия 3: баланс белого на мозаике и обрезка пересвета.
//!
//! Баланс делается до демозаика: алгоритм Menon 2007 опирается на цветоразностные сигналы
//! (R − G, B − G), а они осмысленны, только если серый уже серый.
//!
//! # Усиления
//! Нейтраль камеры `N` (DNG AsShotNeutral: RGB камеры серого объекта, считает наша 3A в 08,
//! пока — HAL) даёт усиления `gain_c = max(N) / N_c`. Нормировка на `max(N)` делает наименьшее
//! усиление равным 1: канал, который насыщается первым, не усиливается.
//!
//! # Пересвет
//! Насыщенный в сенсоре пиксель после усилений стал бы цветным (обычно розовым: R и B
//! усилены, G нет). Поэтому все каналы обрезаются до [`SENSOR_CLIP_LEVEL`] — уровня, при
//! котором насыщается канал с усилением 1 (как режим «clip» в dcraw/LibRaw). Цена — яркие
//! ненасыщенные красные/синие тоже обрезаются; восстановление пересветов — бэклог.

use crate::cfa::RgbChannel;
use crate::color::dng::CameraNeutral;
use crate::image::{ShadingCorrectedMosaic, WhiteBalancedMosaic, mosaic_stage};

/// Уровень обрезки после баланса: белый сенсора в нормированной мозаике (см. модуль).
pub const SENSOR_CLIP_LEVEL: f32 = 1.0;

/// Усиления каналов баланса белого; наименьшее равно 1 по построению.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteBalanceGains {
    gains: [f32; 3],
}

impl WhiteBalanceGains {
    /// Усиления из нейтрали камеры: `max(N) / N_c` (см. модуль).
    pub fn from_camera_neutral(neutral: CameraNeutral) -> Self {
        let values = neutral.values();
        let brightest = values.iter().copied().fold(f64::MIN, f64::max);
        // f64 → f32: усиления применяются к пикселям в f32, как в шейдере.
        Self {
            gains: values.map(|value| (brightest / value) as f32),
        }
    }

    /// Усиление канала.
    pub fn gain(&self, channel: RgbChannel) -> f32 {
        self.gains[channel.index()]
    }

    /// Усиления `[R, G, B]` (для матрицы цвета и для GPU).
    pub fn as_array(&self) -> [f32; 3] {
        self.gains
    }
}

/// Стадия 3: `min(отсчёт · gain[цвет CFA], SENSOR_CLIP_LEVEL)`.
pub fn apply_white_balance(mosaic: &ShadingCorrectedMosaic, gains: &WhiteBalanceGains) -> WhiteBalancedMosaic {
    let pattern = mosaic.pattern();
    mosaic.map_into(mosaic_stage::WhiteBalanced, |x, y, sample| {
        (sample * gains.gain(pattern.channel_at(x, y))).min(SENSOR_CLIP_LEVEL)
    })
}

#[cfg(test)]
#[path = "white_balance_tests.rs"]
mod tests;
