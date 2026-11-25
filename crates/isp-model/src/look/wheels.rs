//! Колёса цвета по зонам (решения 186, 191–193): Lift (тени), Gamma (середина), Gain (света), Offset (весь кадр).
//!
//! Работают в OKLab рабочего пространства `W` над светом экрана ПОСЛЕ основы тона — первым шагом стадии цвета
//! вида ([`super::color::LookColorParams`]), до насыщенности, сочности и полос: сначала «первичная» коррекция
//! по зонам, потом «вторичная» по цветам, как в программах цветокоррекции. От модели основы колёса не зависят.
//!
//! # Зоны — «настоящие» (решение 191)
//! Зоны делятся по светлоте OKLab `L` вокруг светлоты серого экрана `L_g` (из конфига, `L_g = Y_g^(1/3)`: у
//! нормированного OKLab серый `W` с яркостью `Y` даёт ровно `L = Y^(1/3)`):
//! ```text
//! s(t)  = t²·(3 − 2t)                                 — smoothstep, t заранее обрезан в [0, 1]
//! w_sh  = 1 − s(clamp(1 + (L − L_g) / L_g))            — тени: 1 в чёрном, РОВНО 0 на сером и выше
//! w_hi  = s(clamp((L − L_g) / (1 − L_g)))              — света: РОВНО 0 на сером и ниже, 1 в белом
//! w_mid = 1 − w_sh − w_hi                              — середина: 1 на сером, 0 в чёрном и в белом
//! ```
//! Сумма весов всегда 1, переходы гладкие (C1): у весов нет ступенек и изломов. Деление на `L_g` и `1 − L_g`
//! заменено умножением на обратные числа, посчитанные один раз на CPU, — GPU повторяет те же операции.
//!
//! Окно теней записано через `L − L_g`, а не как `L / L_g` (по смыслу одно и то же), ради точного нуля в f32:
//! при `L ≥ L_g` разность неотрицательна точно, `1 + неотрицательное ≥ 1`, обрезка даёт ровно 1, вес теней —
//! ровно 0 при ЛЮБОМ сером из конфига. `L · (1/L_g)` на самом сером могло бы дать 0,99999994 и вес ~10⁻¹³.
//!
//! # Сдвиг пикселя
//! ```text
//! Δ = w_sh·lift + w_mid·gamma + w_hi·gain + offset      — Δ = (ΔL, Δa, Δb), по зонам заранее умножены на силу
//! (L', a', b') = (L + ΔL, a + Δa, b + Δb)
//! ```
//! Толчок цвета колеса `(p_a, p_b)` (точка в круге, `look-contract::WheelPush`) даёт `(Δa, Δb) = C_max·(p_a, p_b)`,
//! кольцо `r ∈ [−1, 1]` — `ΔL = L_max·r`. Веса считаются по светлоте ДО сдвига. Чистый белый и чистый чёрный
//! колёса тоже красят (решение 192), как в DaVinci Resolve; «Честный» (все колёса в нуле) их не трогает —
//! нейтральные колёса пропускаются целиком ([`LookWheels::resolve`] даёт `None`).
//!
//! # Монотонность яркости
//! `dL'/dL = 1 + L_max·Σ r_z·w_z'(L)`. У smoothstep наибольшая крутизна `1,5` на ширину окна, в каждой точке
//! крутится не больше двух окон (тени и середина ниже серого, середина и света выше), и вклад середины равен
//! минус вкладу соседа: `|Σ r_z·w_z'| ≤ 2·1,5 / min(L_g, 1 − L_g)`. Конфиг обязан держать
//! `L_max · 3 / min(L_g, 1 − L_g) < 1` ([`WheelModelSettings::validate`]): тогда на краях колец светлое
//! никогда не становится темнее тёмного (тон не «переворачивается»).

use look_contract::{ColorWheel, ColorWheels};
use serde::{Deserialize, Serialize};

/// Наибольшая крутизна smoothstep на единицу ширины окна (`s'(½) = 1,5`).
const SMOOTHSTEP_MAX_SLOPE: f32 = 1.5;
/// Сколько окон одновременно меняются в одной точке (сосед и середина) — множитель оценки монотонности.
const WINDOWS_CHANGING_AT_ONCE: f32 = 2.0;
/// Наибольшая сила толчка цвета на краю, цветность OKLab: за ней цвет зоны уходит далеко за охват экрана.
const MAX_PUSH_CHROMA_RANGE: f32 = 0.2;
/// Светлота серого экрана должна оставлять обеим сторонам окна хотя бы эту ширину (иначе окно вырождается).
const MIN_ZONE_WIDTH_LIGHTNESS: f32 = 0.1;

/// Конфиг колёс: сила на краю и где проходит граница зон. Все числа — `[estimate]`, подбор — 14b.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WheelModelSettings {
    /// `C_max`: сдвиг цветности OKLab при толчке на краю круга (сила 1). 0,20 — выбор владельца по снимкам 14l:
    /// край — заведомо перебор (решение 246), рабочая зона у центра круга.
    pub push_chroma_range: f32,
    /// `L_max`: сдвиг светлоты OKLab при кольце на краю (±1). 0,135 — почти предел монотонности 0,1384 (см. модуль):
    /// дальше светлое стало бы темнее тёмного, физически не расширить (решение 246).
    pub ring_lightness_range: f32,
    /// Серый экрана, вокруг которого делятся зоны: линейная яркость `Y_g` в `(0, 1)`. `[estimate]`: 0,20 — серый,
    /// в который основы тона по умолчанию переводят серый сцены (`display_middle_gray`).
    ///
    /// Своя настройка, а не `display_middle_gray` выбранной модели и не ручка «средние тона»: слой вида не зависит
    /// от модели основы (решение 182), а зоны считаются по тому, что на экране. Поэтому если сдвинуть серый модели
    /// в конфиге (`render.viewfinder.tone.*.display_middle_gray`), граница зон за ним НЕ пойдёт — сдвигать вместе.
    pub zone_pivot_display_gray: f32,
}

impl Default for WheelModelSettings {
    fn default() -> Self {
        Self {
            push_chroma_range: 0.20,
            ring_lightness_range: 0.135,
            zone_pivot_display_gray: 0.20,
        }
    }
}

/// Конфиг колёс не годится. Варианты различимы: «число за пределами» и «кольца переворачивают тон».
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum InvalidWheelModel {
    /// Число не конечное или вне своего диапазона.
    #[error("wheels.{field} = {value}: нужно конечное число в диапазоне {min}…{max}")]
    OutOfRange {
        /// Поле конфига.
        field: &'static str,
        /// Отвергнутое значение.
        value: f32,
        /// Нижняя граница (включительно).
        min: f32,
        /// Верхняя граница (включительно).
        max: f32,
    },
    /// Кольцо яркости на краю сделало бы тон немонотонным (светлое темнее тёмного).
    #[error("wheels.ring_lightness_range = {value}: при сером {pivot_lightness} (светлота) нужно меньше {limit}")]
    RingBreaksMonotonicity {
        /// Отвергнутая сила кольца.
        value: f32,
        /// Светлота серого экрана `L_g`.
        pivot_lightness: f32,
        /// Строгий предел для этой светлоты.
        limit: f32,
    },
}

impl WheelModelSettings {
    /// Светлота OKLab серого экрана `L_g = Y_g^(1/3)`.
    pub fn pivot_lightness(&self) -> f32 {
        self.zone_pivot_display_gray.cbrt()
    }

    /// Строгий предел силы кольца для текущего серого (см. «Монотонность яркости» в доке модуля).
    pub fn ring_monotonicity_limit(&self) -> f32 {
        let pivot = self.pivot_lightness();
        let narrowest_window = pivot.min(1.0 - pivot);
        narrowest_window / (WINDOWS_CHANGING_AT_ONCE * SMOOTHSTEP_MAX_SLOPE)
    }

    /// Проверяет числа и монотонность тона на краях колец.
    pub fn validate(&self) -> Result<(), InvalidWheelModel> {
        let check = |field: &'static str, value: f32, min: f32, max: f32| {
            if value.is_finite() && value >= min && value <= max {
                Ok(())
            } else {
                Err(InvalidWheelModel::OutOfRange { field, value, min, max })
            }
        };
        check("push_chroma_range", self.push_chroma_range, 0.0, MAX_PUSH_CHROMA_RANGE)?;
        check("ring_lightness_range", self.ring_lightness_range, 0.0, 1.0)?;
        // Серый в таких пределах оставляет обоим окнам ширину ≥ MIN_ZONE_WIDTH_LIGHTNESS по светлоте.
        let min_gray = MIN_ZONE_WIDTH_LIGHTNESS.powi(3);
        let max_gray = (1.0 - MIN_ZONE_WIDTH_LIGHTNESS).powi(3);
        check(
            "zone_pivot_display_gray",
            self.zone_pivot_display_gray,
            min_gray,
            max_gray,
        )?;
        let limit = self.ring_monotonicity_limit();
        if self.ring_lightness_range >= limit {
            return Err(InvalidWheelModel::RingBreaksMonotonicity {
                value: self.ring_lightness_range,
                pivot_lightness: self.pivot_lightness(),
                limit,
            });
        }
        Ok(())
    }
}

/// Сдвиг одной зоны в OKLab на полной силе её веса: `[ΔL, Δa, Δb]`.
pub type ZoneShift = [f32; 3];

/// Колёса, разрешённые в числа кадра (общий вид для CPU и GPU: эти же числа кладутся в uniform).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LookWheels {
    /// `L_g`: светлота серого экрана — граница теней и светов.
    pub pivot_lightness: f32,
    /// `1 / L_g`: масштаб окна теней.
    pub shadow_window_scale: f32,
    /// `1 / (1 − L_g)`: масштаб окна светов.
    pub highlight_window_scale: f32,
    /// Сдвиг теней (Lift).
    pub lift: ZoneShift,
    /// Сдвиг середины (Gamma).
    pub gamma: ZoneShift,
    /// Сдвиг светов (Gain).
    pub gain: ZoneShift,
    /// Сдвиг всего кадра (Offset).
    pub offset: ZoneShift,
}

/// Веса зон пикселя (для тестов и доков: сумма — 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneWeights {
    /// Вес теней.
    pub shadows: f32,
    /// Вес середины.
    pub midtones: f32,
    /// Вес светов.
    pub highlights: f32,
}

impl LookWheels {
    /// Разрешает колёса. `None` — все колёса в нуле: шаг пропускается целиком (нейтраль бит в бит).
    ///
    /// Конфиг уже проверен ([`WheelModelSettings::validate`] вызывается из проверки модели вида).
    pub fn resolve(wheels: &ColorWheels, model: &WheelModelSettings) -> Option<Self> {
        if wheels.is_neutral() {
            return None;
        }
        let pivot = model.pivot_lightness();
        let shift = |wheel: ColorWheel| -> ZoneShift {
            [
                model.ring_lightness_range * wheel.ring.get(),
                model.push_chroma_range * wheel.push.a(),
                model.push_chroma_range * wheel.push.b(),
            ]
        };
        Some(Self {
            pivot_lightness: pivot,
            shadow_window_scale: 1.0 / pivot,
            highlight_window_scale: 1.0 / (1.0 - pivot),
            lift: shift(wheels.lift),
            gamma: shift(wheels.gamma),
            gain: shift(wheels.gain),
            offset: shift(wheels.offset),
        })
    }

    /// Веса зон для светлоты `lightness` (формулы модуля; тот же порядок операций, что в GLSL).
    pub fn zone_weights(&self, lightness: f32) -> ZoneWeights {
        let shadow_t = (1.0 + (lightness - self.pivot_lightness) * self.shadow_window_scale).clamp(0.0, 1.0);
        let highlight_t = ((lightness - self.pivot_lightness) * self.highlight_window_scale).clamp(0.0, 1.0);
        let shadows = 1.0 - smoothstep_unit(shadow_t);
        let highlights = smoothstep_unit(highlight_t);
        ZoneWeights {
            shadows,
            midtones: 1.0 - shadows - highlights,
            highlights,
        }
    }

    /// Сдвигает пиксель OKLab `[L, a, b]`.
    pub fn apply(&self, lab: [f32; 3]) -> [f32; 3] {
        let weights = self.zone_weights(lab[0]);
        std::array::from_fn(|channel| {
            lab[channel]
                + weights.shadows * self.lift[channel]
                + weights.midtones * self.gamma[channel]
                + weights.highlights * self.gain[channel]
                + self.offset[channel]
        })
    }
}

/// `t²·(3 − 2t)` для `t`, уже обрезанного в `[0, 1]` (без своего `clamp`: его делает вызывающий).
fn smoothstep_unit(unit: f32) -> f32 {
    unit * unit * (3.0 - 2.0 * unit)
}
