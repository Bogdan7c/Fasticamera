//! Свой белый у каждой точки (решение 177).
//!
//! После коррекции виньетирования сенсор насыщается в углу не на уровне 1, а на `g'·wb`: насыщенный
//! отсчёт `1,0` усилен картой `g'` (с учётом силы) и усилением баланса `wb`. Обрезка по общему 1,0 срезала бы
//! света в углах, где предмет ярче белого центра, но ещё не насыщен. Поэтому при основе AgX
//! ([`crate::tone::base::WhitePolicy::PerPoint`]):
//! ```text
//! L(pos) = min_c( g'_c(pos) · wb_c )         c — четыре канала CFA [R, G_even, G_odd, B]
//! отсчёт после баланса = min(отсчёт · wb_c, L(pos))
//! ```
//! `g'` берётся той же выборкой, что у стадии виньетирования ([`LensShadingSampling`]): `PerSample` для
//! фото (точка отсчёта), `PerCellCentre` для видоискателя (центр ячейки 2×2). Без коррекции `g' = 1`, а наименьшее
//! усиление баланса равно 1 по построению, значит `L = 1` — поведение прежнее. Всегда `L ≥ 1`.
//!
//! После демозаика белый пикселя изображения — та же функция в той же точке: [`WhiteLevelPlane`].
//! У Preview (суперпиксель 2×2) пиксель — это ячейка, у Quality — пиксель мозаики. GPU считает `L` по
//! карте в шейдере той же формулой и той же выборкой.

use camera_contract::geometry::PixelSize;

use crate::demosaic::DemosaicMethod;
use crate::image::{ShadingCorrectedMosaic, WhiteBalancedMosaic, mosaic_stage};
use crate::lens_shading::{CropInValidArea, LensShadingSampling, LensShadingStep};
use crate::white_balance::WhiteBalanceGains;

/// Функция `L(pos)` для вырезки полезной области: единственное место формулы (её зовут и обрезка баланса,
/// и плоскость белого).
#[derive(Debug, Clone, Copy)]
pub struct WhiteLevelField<'a> {
    step: &'a LensShadingStep,
    gains: &'a WhiteBalanceGains,
    crop: CropInValidArea,
    sampling: LensShadingSampling,
}

impl<'a> WhiteLevelField<'a> {
    /// Поле белого для вырезки `crop` (весь кадр — [`CropInValidArea::whole`]).
    pub fn new(
        step: &'a LensShadingStep,
        gains: &'a WhiteBalanceGains,
        crop: CropInValidArea,
        sampling: LensShadingSampling,
    ) -> Self {
        Self {
            step,
            gains,
            crop,
            sampling,
        }
    }

    /// Белый для отсчёта `(x, y)` мозаики вырезки.
    pub fn level_at(&self, x: u32, y: u32) -> f32 {
        let map_gains = match self.step {
            // Коррекции нет: усиление 1 по всем каналам — формула та же, ответ `min(wb) = 1`.
            LensShadingStep::Skip(_) => [1.0_f32; 4],
            LensShadingStep::Apply(map) => {
                let valid_area = self.crop.valid_area_size();
                map.channel_gains_at(
                    self.crop.valid_area_position(x, y),
                    valid_area,
                    map.grid_scales(valid_area),
                    self.sampling,
                )
            }
        };
        let balance = self.gains.as_array();
        // Каналы карты: R, G_even, G_odd, B; у зелёных усиление баланса одно.
        let saturated = [
            map_gains[0] * balance[0],
            map_gains[1] * balance[1],
            map_gains[2] * balance[1],
            map_gains[3] * balance[2],
        ];
        saturated[0].min(saturated[1]).min(saturated[2]).min(saturated[3])
    }
}

/// Стадия 3 со своим белым: `min(отсчёт · wb_c, L(pos))` вместо общего `SENSOR_CLIP_LEVEL`.
pub fn apply_white_balance_per_point(
    mosaic: &ShadingCorrectedMosaic,
    gains: &WhiteBalanceGains,
    field: &WhiteLevelField<'_>,
) -> WhiteBalancedMosaic {
    let pattern = mosaic.pattern();
    mosaic.map_into(mosaic_stage::WhiteBalanced, |x, y, sample| {
        (sample * gains.gain(pattern.channel_at(x, y))).min(field.level_at(x, y))
    })
}

/// Белый каждого пикселя изображения после демозаика (рядом с изображением, тот же размер).
#[derive(Debug, Clone, PartialEq)]
pub struct WhiteLevelPlane {
    size: PixelSize,
    levels: Vec<f32>,
}

impl WhiteLevelPlane {
    /// Плоскость для изображения после демозаика `method`: Preview — половинное разрешение, белый ячейки;
    /// Quality — полное, белый точки пикселя. `mosaic_size` — размер мозаики (чётный).
    pub fn for_demosaic(field: &WhiteLevelField<'_>, method: DemosaicMethod, mosaic_size: PixelSize) -> Self {
        let (size, step) = match method {
            DemosaicMethod::Preview => (PixelSize::new(mosaic_size.width / 2, mosaic_size.height / 2), 2),
            DemosaicMethod::Quality => (mosaic_size, 1),
        };
        let mut levels = Vec::with_capacity(size.width as usize * size.height as usize);
        for y in 0..size.height {
            for x in 0..size.width {
                levels.push(field.level_at(x * step, y * step));
            }
        }
        Self { size, levels }
    }

    /// Однородная плоскость (белый везде одинаков): для тестов и кадров без коррекции.
    pub fn uniform(size: PixelSize, level: f32) -> Self {
        Self {
            size,
            levels: vec![level; size.width as usize * size.height as usize],
        }
    }

    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Белые по строкам изображения.
    pub fn levels(&self) -> &[f32] {
        &self.levels
    }

    pub fn level(&self, x: u32, y: u32) -> f32 {
        self.levels[y as usize * self.size.width as usize + x as usize]
    }
}

#[cfg(test)]
#[path = "white_level_tests.rs"]
mod tests;
