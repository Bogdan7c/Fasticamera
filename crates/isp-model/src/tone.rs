//! Стадия 6б: глобальная тон-кривая сцена → экран с сохранением оттенка (решение владельца 47).
//!
//! # Кривая
//! Своя параметрическая S-кривая на основе лог-логистической функции (того же семейства, что
//! «sigmoid» в darktable, но формулировка и параметризация — наши):
//!
//! ```text
//! F(x) = x^c / (x^c + s^c)
//! y(x) = min(F(x) / F(x_white), 1)
//! ```
//!
//! - `c` — контраст: наклон кривой в логарифмических осях около среднего серого.
//! - `x_white` — свет сцены, который становится белым экрана (по умолчанию насыщение сенсора).
//! - `s` подбирается так, чтобы средний серый сцены `g` стал заданным серым экрана `y_g`.
//!   Решение в замкнутом виде: при `q = (s / x_white)^c` и `K = (x_white / g)^c` имеем
//!   `y_g = (1 + q) / (1 + K·q)`, откуда `q = (1 − y_g) / (y_g·K − 1)` (нужно `y_g·K > 1`).
//!
//! Свет ярче `x_white` плавно не сжимается, а обрезается до 1: выше насыщения сенсора после
//! баланса белого света нет (см. [`crate::white_balance`]), кроме насыщенных цветов после
//! матрицы — их сводит к белому именно кривая.
//!
//! # Сохранение оттенка
//! Кривая по каналам даёт естественный уход ярких цветов в белый, но сдвигает оттенок
//! (оранжевое желтеет, «notorious six»). Поэтому после кривой оттенок возвращается: каналы
//! упорядочены одинаково до и после (кривая монотонна), а оттенок в шестигранной модели
//! (HSV) задаёт доля среднего канала `(mid − min) / (max − min)`. Её берём со входа, а
//! `max` и `min` — с выхода по каналам. Параметр `hue_preservation` (0…1) смешивает
//! «чисто по каналам» (0) и «оттенок сохранён» (1).

use crate::image::{DisplayLinearImage, SceneInGamutImage, space};

/// Настройки тона (конфиг). Значения по умолчанию — оценка по публичным описаниям
/// («естественный» iPhone: мягкий контраст, чуть светлее середина, мягкие блики) до парных
/// снимков с iPhone 17 Pro Max (решение 25).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneCurveSettings {
    /// Контраст `c` (> 0). 1 — без S-образности в середине; darktable sigmoid по умолчанию 1,5,
    /// его пресет «neutral gray» — 1,22.
    pub contrast: f32,
    /// Средний серый сцены в нормированном свете (0…`scene_white`): какой свет считать 18 %-серым.
    /// Это и есть «экспозиция» обработки (аналог DNG BaselineExposure).
    pub scene_middle_gray: f32,
    /// Во что превращается средний серый на экране (линейный свет 0…1).
    pub display_middle_gray: f32,
    /// Свет сцены, который становится белым экрана.
    pub scene_white: f32,
    /// 0 — кривая по каналам, 1 — оттенок полностью сохранён (решение 47: по умолчанию 1).
    pub hue_preservation: f32,
}

impl Default for ToneCurveSettings {
    fn default() -> Self {
        Self {
            // Между «neutral gray» (1,22) и умолчанием darktable (1,5): мягче, чем у вендора.
            contrast: 1.35,
            // AE вендора оставляет ~2,3 ступени запаса под блики (светло-серая плитка кадра C7
            // «лампа» — Y ≈ 0,20). С 0,075 наш рендер этого кадра совпадает с JPEG вендора по
            // средней яркости (0,458 против 0,462), серой плитке (0,743 / 0,744) и тени (0,196 / 0,195):
            // мы не темнее и не светлее того, под что рассчитана вендорская экспозиция.
            // Уточняется в 08 (наша 3A) и по парным снимкам с iPhone (решение 25).
            scene_middle_gray: 0.075,
            // 0,20 вместо 0,18: середина на ~0,15 ступени светлее (iPhone светлее в средних тонах).
            display_middle_gray: 0.20,
            scene_white: 1.0,
            hue_preservation: 1.0,
        }
    }
}

/// Настройки тона противоречивы.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidToneCurve {
    #[error("контраст {contrast} должен быть конечным и > 0")]
    InvalidContrast { contrast: f32 },
    #[error("средний серый сцены {scene_middle_gray} должен лежать в (0, scene_white = {scene_white})")]
    SceneMiddleGrayOutOfRange { scene_middle_gray: f32, scene_white: f32 },
    #[error("средний серый экрана {display_middle_gray} должен лежать в (0, 1)")]
    DisplayMiddleGrayOutOfRange { display_middle_gray: f32 },
    #[error(
        "с контрастом {contrast} средний серый экрана не может быть ниже {minimum}: кривая стала бы \
         вогнутой во всём диапазоне"
    )]
    DisplayMiddleGrayUnreachable { contrast: f32, minimum: f32 },
    #[error("сохранение оттенка {hue_preservation} вне 0…1")]
    HuePreservationOutOfRange { hue_preservation: f32 },
}

/// Разрешённая кривая: всё, что нужно на пиксель (одинаково для CPU и GPU).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneCurve {
    contrast: f32,
    /// `s^c` — постоянная знаменателя `F`.
    scale_pow_contrast: f32,
    /// `1 / F(x_white)` — нормировка, чтобы `x_white` стал ровно 1.
    white_normalization: f32,
    hue_preservation: f32,
}

impl ToneCurve {
    pub fn resolve(settings: &ToneCurveSettings) -> Result<Self, InvalidToneCurve> {
        validate(settings)?;
        let contrast = f64::from(settings.contrast);
        let scene_white = f64::from(settings.scene_white);
        let display_gray = f64::from(settings.display_middle_gray);
        // K = (x_white / g)^c, q = (1 − y_g) / (y_g·K − 1) — формула модуля.
        let white_to_gray_ratio = (scene_white / f64::from(settings.scene_middle_gray)).powf(contrast);
        let denominator = display_gray * white_to_gray_ratio - 1.0;
        if denominator <= 0.0 {
            return Err(InvalidToneCurve::DisplayMiddleGrayUnreachable {
                contrast: settings.contrast,
                // Сужение до f32 только для текста ошибки.
                minimum: (1.0 / white_to_gray_ratio) as f32,
            });
        }
        let q = (1.0 - display_gray) / denominator;
        // s^c = q · x_white^c.
        let scale_pow_contrast = q * scene_white.powf(contrast);
        let white_pow_contrast = scene_white.powf(contrast);
        let f_white = white_pow_contrast / (white_pow_contrast + scale_pow_contrast);
        Ok(Self {
            contrast: settings.contrast,
            scale_pow_contrast: scale_pow_contrast as f32,
            white_normalization: (1.0 / f_white) as f32,
            hue_preservation: settings.hue_preservation,
        })
    }

    /// Кривая для одного канала: `min(F(x)/F(x_white), 1)`. Отрицательный свет и NaN → 0
    /// (явно: `NaN.min(1.0)` дал бы белый, а шейдер — что угодно).
    pub fn map_channel(&self, scene: f32) -> f32 {
        if scene.is_nan() || scene <= 0.0 {
            return 0.0;
        }
        let powered = scene.powf(self.contrast);
        (powered / (powered + self.scale_pow_contrast) * self.white_normalization).min(1.0)
    }

    /// Кривая для пикселя с сохранением оттенка (см. модуль).
    pub fn map_pixel(&self, scene: [f32; 3]) -> [f32; 3] {
        if scene.iter().any(|channel| channel.is_nan()) {
            // Как в сжатии охвата: NaN в любом канале — чёрный пиксель.
            return [0.0; 3];
        }
        let mut display = scene.map(|channel| self.map_channel(channel));
        let order = channel_order(scene);
        let input_span = scene[order.max] - scene[order.min];
        if input_span <= 0.0 || self.hue_preservation <= 0.0 {
            // Серый пиксель: оттенка нет, сохранять нечего.
            return display;
        }
        let middle_share = (scene[order.middle] - scene[order.min]) / input_span;
        let restored_middle = display[order.min] + middle_share * (display[order.max] - display[order.min]);
        display[order.middle] += self.hue_preservation * (restored_middle - display[order.middle]);
        display
    }
}

fn validate(settings: &ToneCurveSettings) -> Result<(), InvalidToneCurve> {
    let ToneCurveSettings {
        contrast,
        scene_middle_gray,
        display_middle_gray,
        scene_white,
        hue_preservation,
    } = *settings;
    if !(contrast.is_finite() && contrast > 0.0) {
        return Err(InvalidToneCurve::InvalidContrast { contrast });
    }
    if !(scene_white.is_finite() && scene_middle_gray > 0.0 && scene_middle_gray < scene_white) {
        return Err(InvalidToneCurve::SceneMiddleGrayOutOfRange {
            scene_middle_gray,
            scene_white,
        });
    }
    if !(display_middle_gray > 0.0 && display_middle_gray < 1.0) {
        return Err(InvalidToneCurve::DisplayMiddleGrayOutOfRange { display_middle_gray });
    }
    if !(0.0..=1.0).contains(&hue_preservation) {
        return Err(InvalidToneCurve::HuePreservationOutOfRange { hue_preservation });
    }
    Ok(())
}

/// Индексы каналов пикселя по убыванию значения.
struct ChannelOrder {
    max: usize,
    middle: usize,
    min: usize,
}

fn channel_order(pixel: [f32; 3]) -> ChannelOrder {
    let mut indices = [0, 1, 2];
    // Устойчивая сортировка: при равных значениях порядок R, G, B — детерминированно, как на GPU.
    indices.sort_by(|&left, &right| pixel[right].total_cmp(&pixel[left]));
    ChannelOrder {
        max: indices[0],
        middle: indices[1],
        min: indices[2],
    }
}

/// Стадия 6б: сцена → линейный свет экрана той же гаммы.
pub fn apply_tone_curve(image: &SceneInGamutImage, curve: &ToneCurve) -> DisplayLinearImage {
    let gamut = image.space().gamut;
    image.map_into(space::DisplayLinear { gamut }, |pixel| curve.map_pixel(pixel))
}

#[cfg(test)]
#[path = "tone_tests.rs"]
mod tests;
