//! Мягкое сжатие цветов за охватом экрана (решение владельца 47: «мягко приглушаем
//! насыщенность, а не обрезаем»).
//!
//! После матрицы камера → экран очень насыщенные цвета (неон, светодиоды, цветы) дают
//! отрицательный канал — у экрана нет такого цвета. Обрезка канала до 0 меняет оттенок и
//! рвёт плавные переходы. Вместо неё — сжатие по образцу ACES Reference Gamut Compression
//! (Academy, «ACES Gamut Mapping Architecture VWG — Technical Documentation», 2021):
//!
//! ```text
//! ach = max(R, G, B)                  — «ахроматическая ось» пикселя
//! d_c = (ach − c) / ach               — расстояние канала от серого: 0 на сером, 1 на границе охвата,
//!                                       > 1 за охватом (канал отрицателен)
//! d'  = d,                                          если d < threshold
//! d'  = threshold + s·n / (1 + n^p)^(1/p), n = (d − threshold)/s   иначе
//! c'  = ach − d'·ach
//! ```
//!
//! Масштаб `s` подобран так, что расстояние `limit` попадает ровно на границу (d' = 1).
//! Цвета ближе `threshold` к серому не меняются вовсе; между `threshold` и `limit`
//! насыщенность плавно уменьшается; дальше `limit` остаток отрицательного канала обрезается
//! до 0 (страховка, таких пикселей при правильных параметрах мало).
//!
//! Сжатие идёт по каналам независимо, поэтому оттенок сдвигается слегка — это осознанная
//! цена мягкости (как в ACES).

use crate::image::{SceneInGamutImage, SceneLinearImage, space};

/// Параметры сжатия охвата (конфиг; по умолчанию — значения ACES RGC 1.0).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GamutCompressionSettings {
    /// С какого расстояния от серого начинается сжатие, по каналам R, G, B (0…1).
    pub threshold: [f32; 3],
    /// Какое расстояние попадает ровно на границу охвата, по каналам (> 1).
    pub distance_limit: [f32; 3],
    /// Крутизна колена сжатия (≥ 1; больше — резче переход).
    pub power: f32,
}

impl Default for GamutCompressionSettings {
    /// ACES RGC 1.0: пороги 0,815 / 0,803 / 0,880, пределы 1,147 / 1,264 / 1,312, степень 1,2.
    /// Подобраны Academy для типичного выхода цифровых камер за охват; для наших матриц —
    /// оценка, уточняется на реальных кадрах (дневной набор C7).
    fn default() -> Self {
        Self {
            threshold: [0.815, 0.803, 0.880],
            distance_limit: [1.147, 1.264, 1.312],
            power: 1.2,
        }
    }
}

/// Параметры сжатия не имеют смысла.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidGamutCompression {
    #[error("порог канала {channel} = {threshold} вне (0, 1)")]
    ThresholdOutOfRange { channel: usize, threshold: f32 },
    #[error("предел канала {channel} = {limit} должен быть больше 1")]
    LimitNotAboveBoundary { channel: usize, limit: f32 },
    #[error("степень {power} должна быть конечной и не меньше 1")]
    InvalidPower { power: f32 },
}

/// Разрешённые параметры сжатия: пороги, степень и масштаб `s` по каналам.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamutCompression {
    threshold: [f32; 3],
    scale: [f32; 3],
    power: f32,
}

impl GamutCompression {
    pub fn resolve(settings: &GamutCompressionSettings) -> Result<Self, InvalidGamutCompression> {
        let power = settings.power;
        if !power.is_finite() || power < 1.0 {
            return Err(InvalidGamutCompression::InvalidPower { power });
        }
        let mut scale = [0.0; 3];
        let per_channel = settings.threshold.iter().zip(settings.distance_limit.iter());
        for (channel, (output, (&threshold, &limit))) in scale.iter_mut().zip(per_channel).enumerate() {
            if !(threshold > 0.0 && threshold < 1.0) {
                return Err(InvalidGamutCompression::ThresholdOutOfRange { channel, threshold });
            }
            if !(limit.is_finite() && limit > 1.0) {
                return Err(InvalidGamutCompression::LimitNotAboveBoundary { channel, limit });
            }
            *output = compression_scale(threshold, limit, power);
        }
        Ok(Self {
            threshold: settings.threshold,
            scale,
            power,
        })
    }

    /// Сжатое расстояние канала (формула модуля).
    fn compress_distance(&self, channel: usize, distance: f32) -> f32 {
        let threshold = self.threshold[channel];
        if distance < threshold {
            return distance;
        }
        let scale = self.scale[channel];
        let normalized = (distance - threshold) / scale;
        threshold + scale * normalized / (1.0 + normalized.powf(self.power)).powf(1.0 / self.power)
    }

    /// Сжимает один пиксель сцены (линейный свет в основных цветах экрана).
    pub fn compress_pixel(&self, pixel: [f32; 3]) -> [f32; 3] {
        if pixel.iter().any(|channel| channel.is_nan()) {
            // NaN в любом канале — пиксель без смысла: чёрный, одинаково на CPU и GPU.
            return [0.0; 3];
        }
        let achromatic = pixel[0].max(pixel[1]).max(pixel[2]);
        if achromatic <= 0.0 {
            // Нет положительного света ни в одном канале — это чёрный (NaN отсечён выше).
            return [0.0; 3];
        }
        let mut output = [0.0; 3];
        for channel in 0..3 {
            let distance = (achromatic - pixel[channel]) / achromatic;
            let compressed = self.compress_distance(channel, distance);
            // За пределом `limit` сжатое расстояние > 1: остаток отрицательного света обрезаем.
            output[channel] = (achromatic - compressed * achromatic).max(0.0);
        }
        output
    }
}

/// Масштаб `s`, при котором расстояние `limit` переходит ровно в 1 (граница охвата):
/// `s = (limit − threshold) / ((1 − threshold)/(limit − threshold))^(−p) − 1)^(1/p)`.
fn compression_scale(threshold: f32, limit: f32, power: f32) -> f32 {
    let span = limit - threshold;
    span / (((1.0 - threshold) / span).powf(-power) - 1.0).powf(1.0 / power)
}

/// Стадия 6а: сжатие охвата по всему изображению (гамма та же, тип — «внутри охвата»).
pub fn compress_to_display_gamut(image: &SceneLinearImage, compression: &GamutCompression) -> SceneInGamutImage {
    let gamut = image.space().gamut;
    image.map_into(space::SceneInGamut { gamut }, |pixel| compression.compress_pixel(pixel))
}

#[cfg(test)]
#[path = "gamut_tests.rs"]
mod tests;
