//! Стадия «цвет вида»: колёса цвета по зонам, насыщенность, сочность, полосы оттенка — в OKLab/OKLCh рабочего
//! пространства `W`.
//!
//! Работает над светом экрана (display-linear) ПОСЛЕ основы тона, до LUT; от модели основы не зависит
//! (кроме выбора `W`: матрицы OKLab приходят готовыми, [`OklabWorkspace`]).
//!
//! # Формулы пикселя
//! ```text
//! (L, a, b) = OKLab(rgb)
//! (L, a, b) = колёса(L, a, b)            — только если колёса тронуты (super::wheels; иначе шаг пропущен)
//! c² = a² + b²
//! M_sat   = 1 + величина насыщенности                         — множитель цветности (насыщенность)
//! M_vib   = 1 + величина сочности · 1/(1 + c²/c_ref²) · (1 − p·skin)   — сочность
//!   skin  = smoothstep((cosΔ − cos w)/(1 − cos w)),  cosΔ = (a·cos h₀ + b·sin h₀)/√(c² + ε²)
//! g       = c² / (c² + F²)                               — гладкий вес «не серый» (без жёстких порогов)
//! полосы: θ = atan2(b, a) mod 2π;  (Δh, M_b, M_l) = LookBands::effect_at(θ);
//!         φ = g·Δh,  M_band = 1 + g·(M_b − 1),  M_light = 1 + g·(M_l − 1)
//! (a', b') = max(M_sat·M_vib·M_band, 0) · R(φ)·(a, b)        — R — поворот на угол φ
//! L'       = L · M_light
//! rgb'     = OKLab⁻¹(L', a', b')
//! ```
//! Насыщенность и сочность умножают цветность и оттенка не меняют (поворот `R` — только у полос, осознанное
//! исключение из решения 47). Серый: `c → 0` обнуляет `g`, поэтому полосы его не трогают, а множители цветности
//! действуют на остаточные ~10⁻⁴ и ничего не красят.
//!
//! Колёса стоят первыми: сначала коррекция по зонам яркости, потом по цветам (насыщенность и полосы видят уже
//! сдвинутый колёсами цвет). Без колёс операции стадии ровно те же, что до 14i: картинка с одной насыщенностью
//! не меняется ни на бит.
//!
//! Нейтральные ручки цвета пропускают стадию целиком флагом ([`LookColorParams::resolve`] даёт `None`): OKLab туда и
//! обратно не бит-в-бит.
//!
//! Вне `[0, 1]` значения выходят нормально: AgX принимает их выходным сжатием охвата, у прежней основы
//! (свой `W` = цвета выхода) их обрезает `[0, 1]` после стадии.

use look_contract::LookSettings;

use super::bands::{LookBands, resolve_bands};
use super::model::LookModelSettings;
use super::wheels::LookWheels;
use crate::color::chromaticity::Primaries;
use crate::color::matrix::SingularMatrix;
use crate::color::oklab_working::OklabWorkspace;

/// Добавка в знаменатель направления кожи (для `c → 0`), не порог.
const DIRECTION_EPSILON: f32 = 1e-6;

/// Разрешённые числа стадии цвета (общий вид для CPU и GPU).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct LookColorParams {
    /// Матрицы OKLab рабочего пространства.
    pub oklab: OklabWorkspace,
    /// `M_sat`: множитель цветности насыщенности.
    pub saturation_multiplier: f32,
    /// Сила сочности со знаком.
    pub vibrance_amount: f32,
    /// `c_ref`: цветность, при которой вес сочности падает вдвое.
    pub vibrance_chroma_reference: f32,
    /// `(cos h₀, sin h₀)` центра окна кожи в плоскости `(a, b)`.
    pub skin_direction: [f32; 2],
    /// `cos w` — косинус полуширины окна кожи.
    pub skin_cos_half_width: f32,
    /// `p`: доля подавления сочности в центре окна.
    pub skin_protection: f32,
    /// `F`: гладкий порог серого.
    pub neutral_chroma_floor: f32,
    /// Полосы оттенка; `None` — стадия полос пропущена.
    pub bands: Option<LookBands>,
    /// Колёса цвета по зонам; `None` — шаг колёс пропущен.
    pub wheels: Option<LookWheels>,
}

/// Результат разрешения стадии: параметры (или пропуск) и неизвестные модели полосы.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedColor {
    /// `None` — нейтрально, стадия пропускается целиком.
    pub params: Option<LookColorParams>,
    /// Идентификаторы правок полос, которых нет в модели.
    pub unknown_band_ids: Vec<look_contract::HueBandId>,
}

impl LookColorParams {
    /// Разрешает стадию для рабочего пространства `working`. Нейтральность судится ПОСЛЕ разрешения: правки
    /// только неизвестных полос не включают стадию.
    pub fn resolve(
        settings: &LookSettings,
        model: &LookModelSettings,
        working: &Primaries,
    ) -> Result<ResolvedColor, SingularMatrix> {
        let resolved_bands = resolve_bands(model, &settings.bands);
        let wheels = LookWheels::resolve(&settings.wheels, &model.wheels);
        let saturation_neutral = settings.saturation.is_neutral();
        let vibrance_neutral = settings.vibrance.is_neutral();
        if saturation_neutral && vibrance_neutral && resolved_bands.bands.is_none() && wheels.is_none() {
            return Ok(ResolvedColor {
                params: None,
                unknown_band_ids: resolved_bands.unknown_band_ids,
            });
        }
        let skin_center = model.skin.hue_center_degrees.to_radians();
        let params = Self {
            oklab: OklabWorkspace::new(working)?,
            saturation_multiplier: 1.0 + model.saturation.amount(settings.saturation),
            vibrance_amount: model.vibrance.amount(settings.vibrance),
            vibrance_chroma_reference: model.vibrance_chroma_reference,
            skin_direction: [skin_center.cos(), skin_center.sin()],
            skin_cos_half_width: model.skin.half_width_degrees.to_radians().cos(),
            skin_protection: model.skin.protection,
            neutral_chroma_floor: model.neutral_chroma_floor,
            bands: resolved_bands.bands,
            wheels,
        };
        Ok(ResolvedColor {
            params: Some(params),
            unknown_band_ids: resolved_bands.unknown_band_ids,
        })
    }

    /// Вес защиты кожи `0…1` для направления `(a, b)` (гладкая ступенька по косинусу угла до центра окна).
    fn skin_window(&self, a: f32, b: f32, chroma_squared: f32) -> f32 {
        let norm = (chroma_squared + DIRECTION_EPSILON * DIRECTION_EPSILON).sqrt();
        let cosine = (a * self.skin_direction[0] + b * self.skin_direction[1]) / norm;
        let unit = ((cosine - self.skin_cos_half_width) / (1.0 - self.skin_cos_half_width)).clamp(0.0, 1.0);
        unit * unit * (3.0 - 2.0 * unit)
    }

    /// Применяет стадию к свету `W` одного пикселя (формулы модуля).
    pub fn apply(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        let mut lab = self.oklab.to_lab(working_rgb);
        if let Some(wheels) = &self.wheels {
            lab = wheels.apply(lab);
        }
        let [lightness, a, b] = lab;
        let chroma_squared = a * a + b * b;

        let reference_squared = self.vibrance_chroma_reference * self.vibrance_chroma_reference;
        let skin = self.skin_window(a, b, chroma_squared);
        let vibrance_weight = (1.0 - self.skin_protection * skin) / (1.0 + chroma_squared / reference_squared);
        let mut chroma_multiplier = self.saturation_multiplier * (1.0 + self.vibrance_amount * vibrance_weight);

        let (mut rotated_a, mut rotated_b, mut lightness_multiplier) = (a, b, 1.0_f32);
        if let Some(bands) = &self.bands {
            let floor_squared = self.neutral_chroma_floor * self.neutral_chroma_floor;
            let not_grey = chroma_squared / (chroma_squared + floor_squared);
            let hue = b.atan2(a);
            let effect = bands.effect_at(if hue < 0.0 { hue + std::f32::consts::TAU } else { hue });
            let angle = not_grey * effect.hue_shift_radians;
            let (sine, cosine) = angle.sin_cos();
            rotated_a = a * cosine - b * sine;
            rotated_b = a * sine + b * cosine;
            chroma_multiplier *= 1.0 + not_grey * (effect.chroma_multiplier - 1.0);
            lightness_multiplier = 1.0 + not_grey * (effect.lightness_multiplier - 1.0);
        }
        let chroma_multiplier = chroma_multiplier.max(0.0);
        self.oklab.to_rgb([
            lightness * lightness_multiplier,
            rotated_a * chroma_multiplier,
            rotated_b * chroma_multiplier,
        ])
    }
}
