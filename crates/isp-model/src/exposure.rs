//! Остаточная экспозиционная подтяжка ISP и веса яркости для статистики до баланса белого
//! (сессия 08a, решения 63, 66, 68).
//!
//! # Подтяжка `k`
//! Пока сенсор догоняет новую экспозицию (результат виден через 7–8 кадров, шаги квантованы),
//! ISP обязан показывать кадр так, как будто экспозиция уже верная: свет сцены домножается на
//! `k = D / A` (D — что должно быть на экране, A — «выдержка × ISO», реально стоявшая на кадре).
//!
//! Подтяжка применяется **через тон**, а не множителем пикселей. Если умножить пиксели на `k`,
//! то белый сенсора (1,0) при `k < 1` станет серым, а при `k > 1` уйдёт за белый раньше времени:
//! клипнутые блики — это белый, а не «яркость, которую можно сдвинуть». Поэтому `k` меняет только
//! положение среднего серого кривой (`scene_middle_gray / k`), а `scene_white` остаётся прежним:
//! кривая нормирована так, что белый сенсора — ровно белый экрана при любом `k`. Для остального
//! диапазона это эквивалентно умножению входа тона на `k`. Шейдер GPU не меняется: он получает те
//! же по смыслу числа кривой ([`crate::tone::ToneCurveParameters`]).
//!
//! # Веса яркости
//! Статистика AE считается по мозаике **до** баланса белого (общая с AWB), а тон видит яркость
//! **после** баланса и матрицы. [`LuminanceWeights`] переводят несбалансированные средние R, G, B
//! в ту же яркость: `Y = w_r·R + w_g·G + w_b·B`.

/// Подтяжка экспозиции не годится.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidExposureGain {
    /// Множитель NaN, бесконечный, нулевой или отрицательный (логарифм и деление невозможны).
    #[error("подтяжка экспозиции {value}: нужно конечное положительное число")]
    NotPositiveFinite {
        /// Что передали.
        value: f32,
    },
}

/// Множитель света сцены перед тоном: `1` — без подтяжки, `2` — на +1 EV светлее.
/// Значение всегда конечное и положительное.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct ExposureGain(f32);

impl ExposureGain {
    /// Без подтяжки (прежняя обработка бит в бит).
    pub const UNITY: ExposureGain = ExposureGain(1.0);

    /// Подтяжка из множителя.
    pub fn new(multiplier: f32) -> Result<Self, InvalidExposureGain> {
        if multiplier.is_finite() && multiplier > 0.0 {
            Ok(Self(multiplier))
        } else {
            Err(InvalidExposureGain::NotPositiveFinite { value: multiplier })
        }
    }

    /// Подтяжка из ступеней: `2^ev`. Переполнение f32 (бесконечность) — ошибка.
    pub fn from_ev(ev: f32) -> Result<Self, InvalidExposureGain> {
        Self::new(ev.exp2())
    }

    /// Множитель света.
    pub fn multiplier(self) -> f32 {
        self.0
    }

    /// Подтяжка в ступенях (`log2`).
    pub fn ev(self) -> f32 {
        self.0.log2()
    }
}

/// Веса `[w_r, w_g, w_b]`: яркость тона из **несбалансированных** средних R, G (среднее двух
/// зелёных), B. Получает [`crate::params::IspParams::scene_luminance_weights`].
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct LuminanceWeights {
    r: f32,
    g: f32,
    b: f32,
}

impl LuminanceWeights {
    /// Веса из трёх значений (для теста и конфигураций; рабочие берутся из `IspParams`).
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// Вес красного.
    pub fn r(self) -> f32 {
        self.r
    }

    /// Вес зелёного (применяется к среднему двух зелёных отсчётов).
    pub fn g(self) -> f32 {
        self.g
    }

    /// Вес синего.
    pub fn b(self) -> f32 {
        self.b
    }

    /// Яркость по несбалансированным средним каналам.
    pub fn luminance(self, mean_r: f32, mean_g: f32, mean_b: f32) -> f32 {
        self.r * mean_r + self.g * mean_g + self.b * mean_b
    }
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
