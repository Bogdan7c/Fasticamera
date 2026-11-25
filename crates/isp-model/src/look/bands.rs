//! Полосы оттенка: гладкие окна по кругу OKLCh (решение 99, «ручки по цветам»).
//!
//! # Окна
//! Центры полос `c₀ < c₁ < … < c_{n−1}` на круге `[0, 2π)` (радианы). Оттенок пикселя `θ = atan2(b, a)` mod `2π`
//! лежит между двумя соседними центрами `c_i` и `c_{i+1}` (после последнего — снова `c₀ + 2π`, поэтому переход
//! 360° → 0° ничем не выделен). Вес следующей полосы — приподнятый косинус
//! ```text
//! t = (θ − c_i) / (c_{i+1} − c_i),   w_{i+1} = (1 − cos(π·t)) / 2,   w_i = 1 − w_{i+1}
//! ```
//! Сумма весов всегда 1 (разбиение единицы), на центре вес своей полосы 1, а производная веса там 0: окна C¹.
//! Эффект в точке — взвешенная сумма значений двух соседних полос, значит между центрами он плавно
//! перетекает и у нейтральной полосы равен нейтрали. Одна полоса (`n = 1`) действует на весь круг одинаково.
//!
//! # Формат для шейдера
//! Фиксированные массивы по [`MAX_LOOK_BANDS`] элементов, `count` — сколько занято; центры по возрастанию.

use crate::look::model::{HueBandDefinition, LookModelSettings};
use look_contract::{BandAdjustments, HueBandId};
use std::f32::consts::{PI, TAU};

/// Наибольшее число полос: размер массивов в uniform шейдера.
pub const MAX_LOOK_BANDS: usize = 16;

/// Эффекты полос в точке круга: сдвиг оттенка (радианы), множители цветности и светлоты.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandEffect {
    pub hue_shift_radians: f32,
    pub chroma_multiplier: f32,
    pub lightness_multiplier: f32,
}

/// Разрешённые полосы (общий вид для CPU и GPU).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct LookBands {
    /// Занято элементов в массивах (1…[`MAX_LOOK_BANDS`]).
    pub count: u32,
    /// Центры полос, радианы, по возрастанию, `[0, 2π)`.
    pub centers_radians: [f32; MAX_LOOK_BANDS],
    /// Сдвиг оттенка полосы, радианы.
    pub hue_shift_radians: [f32; MAX_LOOK_BANDS],
    /// Множитель цветности полосы (1 — нейтраль).
    pub chroma_multiplier: [f32; MAX_LOOK_BANDS],
    /// Множитель светлоты полосы (1 — нейтраль).
    pub lightness_multiplier: [f32; MAX_LOOK_BANDS],
}

/// Итог разрешения полос.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBands {
    /// `None`, если ни одна известная модели полоса не тронута: стадия полос пропускается целиком.
    pub bands: Option<LookBands>,
    /// Идентификаторы правок, которых нет в модели (игнорируются; для диагностики).
    pub unknown_band_ids: Vec<HueBandId>,
}

impl LookBands {
    /// Эффекты в точке круга `hue_radians ∈ [0, 2π)` (формулы модуля).
    pub fn effect_at(&self, hue_radians: f32) -> BandEffect {
        let count = self.count as usize;
        // Последний центр, не правее оттенка; левее первого центра — последняя полоса (через круг).
        let mut lower = count - 1;
        for index in 0..count {
            if self.centers_radians[index] <= hue_radians {
                lower = index;
            }
        }
        let upper = (lower + 1) % count;
        let lower_center = self.centers_radians[lower];
        let mut span = self.centers_radians[upper] - lower_center;
        if span <= 0.0 {
            span += TAU;
        }
        let mut offset = hue_radians - lower_center;
        if offset < 0.0 {
            offset += TAU;
        }
        let upper_weight = 0.5 * (1.0 - (PI * (offset / span)).cos());
        let mix = |values: &[f32; MAX_LOOK_BANDS]| values[lower] * (1.0 - upper_weight) + values[upper] * upper_weight;
        BandEffect {
            hue_shift_radians: mix(&self.hue_shift_radians),
            chroma_multiplier: mix(&self.chroma_multiplier),
            lightness_multiplier: mix(&self.lightness_multiplier),
        }
    }
}

/// Разрешает полосы модели и правки пользователя.
pub fn resolve_bands(model: &LookModelSettings, adjustments: &BandAdjustments) -> ResolvedBands {
    let mut definitions: Vec<&HueBandDefinition> = model.bands.iter().collect();
    definitions.sort_by(|left, right| left.center_degrees.total_cmp(&right.center_degrees));
    let unknown_band_ids = adjustments
        .iter()
        .filter(|adjustment| !model.bands.iter().any(|band| band.id == adjustment.band))
        .map(|adjustment| adjustment.band.clone())
        .collect();

    let mut bands = LookBands {
        count: definitions.len() as u32,
        centers_radians: [0.0; MAX_LOOK_BANDS],
        hue_shift_radians: [0.0; MAX_LOOK_BANDS],
        chroma_multiplier: [1.0; MAX_LOOK_BANDS],
        lightness_multiplier: [1.0; MAX_LOOK_BANDS],
    };
    let mut any_touched = false;
    for (slot, definition) in definitions.iter().enumerate() {
        let adjustment = adjustments.for_band(&definition.id);
        any_touched |= !adjustment.is_neutral();
        bands.centers_radians[slot] = definition.center_degrees.to_radians();
        bands.hue_shift_radians[slot] = model.band_hue_degrees.amount(adjustment.hue).to_radians();
        bands.chroma_multiplier[slot] = 1.0 + model.band_saturation.amount(adjustment.saturation);
        bands.lightness_multiplier[slot] = 1.0 + model.band_luminance.amount(adjustment.luminance);
    }
    ResolvedBands {
        bands: any_touched.then_some(bands),
        unknown_band_ids,
    }
}
