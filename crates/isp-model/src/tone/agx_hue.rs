//! Возврат оттенка в основе AgX (решение 180): после S-кривой и `y^γ`, до матрицы «наружу».
//!
//! # Зачем
//! Кривая по каналам уводит яркие насыщенные цвета: красный в пурпур (на 31–45°), оранжевый на 10°. Цель
//! решения 47 («закат не желтеет, кожа не уходит в жёлтое») возвращается шагом: оттенок OKLCh цвета ПОСЛЕ кривой
//! разворачивается к оттенку цвета ДО кривой (после матрицы «внутрь»), а светлота и цветность остаются с
//! выхода кривой.
//!
//! # Формулы (без `atan2`, `sin`, `cos`: паритет GPU надёжнее на одних сложениях, умножениях и `sqrt`)
//! Пусть `before` — цвет до кривой, `after` — после (линейный свет в `W`), `Y` — строка яркости `W`.
//! ```text
//! k      = Y(after) / max(Y(before), 1e-3·Y(after) + 1e-30)     — приведение к яркости выхода кривой
//! lab₀   = OKLab(before·k),   lab₁ = OKLab(after)                — цвет «до» теперь на яркости «после»
//! c₀² = a₀² + b₀²,  c₁ = hypot(a₁, b₁)
//! s      = h · c₀² / (c₀² + F²)                                   — h — степень, F — гладкий порог серого
//! u₀ = (a₀, b₀) / sqrt(c₀² + ε²),   u₁ = (a₁, b₁) / sqrt(c₁² + ε²)    — «почти единичные» направления
//! d      = (1 − s)·u₁ + s·u₀                                     — nlerp: линейная смесь и нормировка
//! (a, b) = c₁ · d / |d|                                          — цветность с выхода кривой, направление d
//! out    = OKLab⁻¹(L₁, a, b)
//! ```
//! Приведение `k` на сам оттенок не влияет (OKLab однороден: умножение света на `k > 0` умножает `L, a, b` на
//! `k^(1/3)`), но задаёт масштаб цветности `c₀`, с которым сравнивается порог серого `F`.
//!
//! * `h = 1`, цвет заметно цветной (`s ≈ 1`): `d = u₀`, оттенок ровно «до кривой».
//! * Около серого `c₀ → 0` вес `s → 0` плавно (без `if c < eps`), `d = u₁`: оттенок остаётся выходным.
//! * Частичная степень — nlerp единичных векторов, а не дуга окружности: угол растёт с `h` неравномерно (быстрее
//!   в середине), крайние точки совпадают с дугой. Вырожденный случай: векторы противоположны и `s = ½` дают
//!   `d = 0` — тогда берётся выходное направление (`(a₁, b₁)` как есть).
//! * Чёрный: `Y(after) = 0` даёт `k = 0`, цветность 0, `out = after`.
//!
//! Степень 0 шаг пропускает целиком ([`super::agx::AgxBase`] не создаёт [`HueRestoration`]) — выход бит в бит.

use super::agx_settings::AgxSettings;
use crate::color::chromaticity::Primaries;
use crate::color::matrix::SingularMatrix;
use crate::color::oklab_working::OklabWorkspace;
use crate::color::working_space::WORKING_PRIMARIES;

/// Малая добавка к знаменателю нормировки направления (для `c → 0`), не порог.
const DIRECTION_EPSILON: f32 = 1e-6;
/// Ниже этой длины смеси направлений («векторы противоположны») берём выходное направление.
const DEGENERATE_BLEND_LENGTH: f32 = 1e-6;
/// Ограничение приведения яркости: `k ≤ 1000`.
const MAX_LUMINANCE_RATIO_INVERSE: f32 = 1e-3;
/// Защита знаменателя от нуля (абсолютная малость, много меньше любой видимой яркости).
const TINY_LUMINANCE: f32 = 1e-30;

/// Числа возврата оттенка для шейдера (только чтение; строятся из `AgxBase::resolve`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct HueRestorationParameters {
    /// Степень `h ∈ (0, 1]`.
    pub degree: f32,
    /// Гладкий порог серого `F` (цветность OKLab).
    pub chroma_floor: f32,
    /// Строка яркости `Y` рабочего пространства: `Y = row · rgb`.
    pub luminance_row: [f32; 3],
    /// Матрицы OKLab для `W`.
    pub oklab: OklabWorkspace,
}

/// Разрешённый шаг возврата оттенка.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HueRestoration {
    parameters: HueRestorationParameters,
}

impl HueRestoration {
    /// `None`, если степень 0: шаг пропускается целиком.
    pub(super) fn resolve(settings: &AgxSettings) -> Result<Option<Self>, SingularMatrix> {
        if settings.hue_restoration <= 0.0 {
            return Ok(None);
        }
        let primaries: &Primaries = &WORKING_PRIMARIES;
        let luminance_row = primaries.rgb_to_xyz()?.rows[1].map(|value| value as f32);
        Ok(Some(Self {
            parameters: HueRestorationParameters {
                degree: settings.hue_restoration,
                chroma_floor: settings.hue_restoration_chroma_floor,
                luminance_row,
                oklab: OklabWorkspace::new(primaries)?,
            },
        }))
    }

    pub(super) fn parameters(&self) -> HueRestorationParameters {
        self.parameters
    }

    fn luminance(&self, rgb: [f32; 3]) -> f32 {
        let row = self.parameters.luminance_row;
        row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2]
    }

    /// Возвращает оттенок `before` цвету `after` (формулы модуля). Оба — линейный свет в `W`.
    pub(super) fn restore(&self, before: [f32; 3], after: [f32; 3]) -> [f32; 3] {
        let after_luminance = self.luminance(after);
        let before_luminance = self.luminance(before);
        let scale =
            after_luminance / before_luminance.max(MAX_LUMINANCE_RATIO_INVERSE * after_luminance + TINY_LUMINANCE);
        let oklab = &self.parameters.oklab;
        let [_, before_a, before_b] = oklab.to_lab(before.map(|channel| channel * scale));
        let [lightness, after_a, after_b] = oklab.to_lab(after);

        let before_chroma_squared = before_a * before_a + before_b * before_b;
        let floor = self.parameters.chroma_floor;
        let weight = self.parameters.degree * before_chroma_squared / (before_chroma_squared + floor * floor);

        let before_norm = (before_chroma_squared + DIRECTION_EPSILON * DIRECTION_EPSILON).sqrt();
        let after_chroma_squared = after_a * after_a + after_b * after_b;
        let after_norm = (after_chroma_squared + DIRECTION_EPSILON * DIRECTION_EPSILON).sqrt();
        let blend_a = (1.0 - weight) * after_a / after_norm + weight * before_a / before_norm;
        let blend_b = (1.0 - weight) * after_b / after_norm + weight * before_b / before_norm;
        let blend_length = blend_a.hypot(blend_b);

        let (new_a, new_b) = if blend_length > DEGENERATE_BLEND_LENGTH {
            let after_chroma = after_chroma_squared.sqrt();
            (
                blend_a * after_chroma / blend_length,
                blend_b * after_chroma / blend_length,
            )
        } else {
            (after_a, after_b)
        };
        oklab.to_rgb([lightness, new_a, new_b])
    }
}
