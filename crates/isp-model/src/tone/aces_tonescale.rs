//! Тональная кривая ACES по каналу: форма осей, спуск по сплайнам RRT и ODT, плавный подход к белому точки.
//!
//! Источник сплайнов и лицензия — в доке модуля [`super`] (`tone::aces`). Здесь — наша часть: как кривая
//! ACES приведена к решениям 72 и 177 (серый в заданный серый экрана; свой белый точки — ровно белый экрана).
//!
//! # Канал за каналом (вход `v` — значение канала AP1 после «подслащивания», ACES: серая карта = 0,18)
//! ```text
//! e   = log2(max(v, HALF_MIN)/0,18)               — ступени от серой карты ACES
//! e₁  = c·e + A·e²/(e² + w²)                      — ручки: контраст c; A = сдвиг теней (e < 0) или света (e ≥ 0)
//! e₁ ≥ E_L  →  u = 1                              — белый точки
//! e₁ ≥ k    →  u = Эрмит(e₁; k, u_k, s_k; E_L, 1, 0)   — колено → белый точки, наклон в белом 0 (C1)
//! e₁ < k    →  u = (10^ODT(RRT(0,18·2^(e₁ + δ))) − Y_min)/(Y_max − Y_min)   — кривая ACES, δ = сдвиг экспозиции
//! ```
//! * `δ` (`shift_ev`) — сдвиг экспозиции модели: подобран так, что `e₁ = 0` (серый сцены) даёт ровно
//!   `display_middle_gray` экрана. Подбор — решение уравнения на CPU по ОБРАТНЫМ сплайнам (замкнутая форма, без
//!   итераций): серый `0,18` у чистого ACES даёт на экране ≈ 0,10, нам нужен заданный.
//! * `k` (`knee_ev`), `u_k`, `s_k` — колено: значение и наклон кривой ACES в `e₁ = k` (на CPU, f64). Всё ниже колена —
//!   ровно ACES и от белого точки не зависит, в частности серый не зависит от `L`.
//! * `E_L = min(формаТеней+Света(log2(L/grey_eff)), E_cap)` — белый точки в ступенях. Эрмитова кубика
//!   `H(e₁) = h₀₀·u_k + h₁₀·(E_L − k)·s_k + h₀₁·1` соединяет колено с белым: значение `1` и наклон `0` в `E_L`
//!   (гладко в зажим). Она монотонна, пока `s_k·(E_L − k) ≤ 3·(1 − u_k)`; предел `E_cap = k + 3(1 − u_k)/s_k`
//!   гарантирует это при любом `L` (белый выше предела достигается раньше, чем на `L`).
//! * Кривая выше ACES-белого (`6,5` ступеней над `0,18`) не используется: колено обязано лежать ниже
//!   (проверка при разрешении).
//!
//! Спуск идёт в `log10` без промежуточных `pow10`: выход RRT (`log10 y`) прямо подаётся входом ODT.

use super::settings::{AcesSettings, InvalidAces};
use super::spline::{HALF_MIN, SplineParameters, SplineSpec};
use crate::tone::agx::MIN_WHITE_LEVEL;
use serde::Serialize;

/// Серая карта ACES: `0,18`.
pub const REFERENCE_GREY: f32 = 0.18;
/// Белый выходного преобразования лежит на `0,18·2^6,5` (`ODT_48nits.maxPoint`), ступеней над серой картой.
const ODT_WHITE_EV: f64 = 6.5;
/// Колено обязано быть ниже белого ACES не менее чем на столько ступеней (кубика не упирается в плоский конец).
const KNEE_MARGIN_EV: f64 = 0.25;
/// Наименьший запас белого точки (`L = 1`) над коленом, ступеней: уже — кубика теряет смысл.
pub const MIN_WHITE_HEADROOM_EV: f64 = 0.25;
/// Предел белого точки в ступенях, если наклон в колене почти нулевой (защита от бесконечности).
const WHITE_EV_CEILING: f64 = 64.0;
/// Шаг центральной разности для наклона в колене (f64; сплайн непрерывно дифференцируем).
const SLOPE_PROBE_EV: f64 = 1e-6;
/// Чёрный и белый нормировки выходного преобразования 48 нит (нит): `linCV = (Y − Y_min)/(Y_max − Y_min)`.
const CINEMA_BLACK_NITS: f64 = 0.02;
const CINEMA_WHITE_NITS: f64 = 48.0;

/// Числа тональной кривой для шейдера: ровно те f32, что подставляет [`ToneScaleParameters::linear_cv`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ToneScaleParameters {
    /// Средний серый сцены с подтяжкой AE (`scene_middle_gray / k`): от него считается белый точки.
    pub grey: f32,
    /// Контраст `c`.
    pub contrast: f32,
    /// Сдвиг теней `A` при `e < 0` и света при `e ≥ 0`, ступеней.
    pub shadows_ev: f32,
    pub highlights_ev: f32,
    /// Ширина перехода `w`.
    pub shape_width_ev: f32,
    /// `log10(0,18)`.
    pub log_reference: f32,
    /// Сдвиг экспозиции `δ`, ступеней.
    pub shift_ev: f32,
    /// Колено: `k`, значение `u_k`, наклон `s_k` (на ступень) и предел белого `E_cap`.
    pub knee_ev: f32,
    pub knee_value: f32,
    pub knee_slope: f32,
    pub max_white_ev: f32,
    /// Нормировка `linCV`: `Y_min` и `1/(Y_max − Y_min)`; `log_black = log10(Y_min)` — ниже него чёрный ровно 0.
    pub black_level: f32,
    pub inverse_range: f32,
    pub log_black: f32,
    pub rrt: SplineParameters,
    pub odt: SplineParameters,
}

/// Форма осей экспозиции (ручки): формула модуля в f64 — для разрешения параметров.
fn shape_exposure(settings: &AcesSettings, e: f64) -> f64 {
    let squared = e * e;
    let width_squared = f64::from(settings.shape_width_ev).powi(2);
    let sweep = if e < 0.0 {
        f64::from(settings.shadows_ev)
    } else {
        f64::from(settings.highlights_ev)
    };
    f64::from(settings.contrast) * e + sweep * squared / (squared + width_squared)
}

impl ToneScaleParameters {
    /// Разрешает кривую (настройки уже с подтяжкой AE, см. [`AcesSettings::with_exposure_gain`]).
    pub fn resolve(settings: &AcesSettings) -> Result<Self, InvalidAces> {
        settings.validate_ranges()?;
        let rrt = SplineSpec::rrt();
        let odt = SplineSpec::odt_48_nits(&rrt);
        let log_reference = f64::from(REFERENCE_GREY).log10();
        let log_black = CINEMA_BLACK_NITS.log10();
        let range = CINEMA_WHITE_NITS - CINEMA_BLACK_NITS;

        // Сдвиг экспозиции: нейтраль (g, g, g) дальше идёт как `g^γ` (поправка окружения) — решаем в обратную сторону.
        let target_cv = f64::from(settings.display_middle_gray).powf(1.0 / f64::from(settings.surround_gamma));
        let log_nits = (CINEMA_BLACK_NITS + target_cv * range).log10();
        let log_input = rrt.reverse_log10(odt.reverse_log10(log_nits));
        let shift_ev = (log_input - log_reference) / std::f64::consts::LOG10_2;

        // Значение кривой ACES (до колена) на оси e₁: `linCV` одной нейтральной компоненты.
        let aces_value = |exposure: f64| -> f64 {
            let log_x = log_reference + (exposure + shift_ev) * std::f64::consts::LOG10_2;
            let log_y = odt.forward_log10(rrt.forward_log10(log_x));
            if log_y <= log_black {
                0.0
            } else {
                ((10.0_f64.powf(log_y) - CINEMA_BLACK_NITS) / range).clamp(0.0, 1.0)
            }
        };

        let knee_ev = f64::from(settings.knee_ev);
        let limit_ev = ODT_WHITE_EV - KNEE_MARGIN_EV - shift_ev;
        if knee_ev >= limit_ev {
            return Err(InvalidAces::KneeBeyondToneScale {
                knee_ev: settings.knee_ev,
                shift_ev: shift_ev as f32,
                limit_ev: limit_ev as f32,
            });
        }
        let knee_value = aces_value(knee_ev);
        let knee_slope =
            (aces_value(knee_ev + SLOPE_PROBE_EV) - aces_value(knee_ev - SLOPE_PROBE_EV)) / (2.0 * SLOPE_PROBE_EV);
        if !(knee_slope > 0.0 && knee_slope.is_finite()) {
            return Err(InvalidAces::KneeSlopeNotPositive {
                knee_ev: settings.knee_ev,
                slope: knee_slope as f32,
            });
        }
        let max_white_ev = (knee_ev + 3.0 * (1.0 - knee_value) / knee_slope).min(WHITE_EV_CEILING);

        // Запас белого точки: самое низкое `L = 1` обязано лежать выше колена.
        let minimal_white_ev = shape_exposure(
            settings,
            (f64::from(MIN_WHITE_LEVEL) / f64::from(settings.scene_middle_gray)).log2(),
        );
        if minimal_white_ev - knee_ev < MIN_WHITE_HEADROOM_EV {
            return Err(InvalidAces::WhiteTooCloseToKnee {
                white_ev: minimal_white_ev as f32,
                knee_ev: settings.knee_ev,
                minimum_headroom_ev: MIN_WHITE_HEADROOM_EV as f32,
            });
        }
        Ok(Self {
            grey: settings.scene_middle_gray,
            contrast: settings.contrast,
            shadows_ev: settings.shadows_ev,
            highlights_ev: settings.highlights_ev,
            shape_width_ev: settings.shape_width_ev,
            log_reference: log_reference as f32,
            shift_ev: shift_ev as f32,
            knee_ev: settings.knee_ev,
            knee_value: knee_value as f32,
            knee_slope: knee_slope as f32,
            max_white_ev: max_white_ev as f32,
            black_level: CINEMA_BLACK_NITS as f32,
            inverse_range: (1.0 / range) as f32,
            log_black: log_black as f32,
            rrt: rrt.parameters(),
            odt: odt.parameters(),
        })
    }

    /// Форма осей: `e → e₁` (ручки контраста, теней и света; формула модуля в f32).
    pub fn shape(&self, e: f32) -> f32 {
        let squared = e * e;
        let sweep = if e < 0.0 { self.shadows_ev } else { self.highlights_ev };
        self.contrast * e + sweep * (squared / (squared + self.shape_width_ev * self.shape_width_ev))
    }

    /// Белый точки в ступенях `E_L` для уровня белого `white` (≥ 1): после формы осей и предела `E_cap`.
    pub fn white_ev(&self, white: f32) -> f32 {
        let exposure = (white.max(MIN_WHITE_LEVEL) / self.grey).log2();
        self.shape(exposure).min(self.max_white_ev)
    }

    /// `linCV` одного канала AP1 (формула модуля) при белом точки `white_ev` (см. [`Self::white_ev`]).
    pub fn linear_cv(&self, value: f32, white_ev: f32) -> f32 {
        let exposure = self.shape((value.max(HALF_MIN as f32) / REFERENCE_GREY).log2());
        if exposure >= white_ev {
            return 1.0;
        }
        if exposure >= self.knee_ev {
            return self.approach_to_white(exposure, white_ev);
        }
        let log_x = self.log_reference + (exposure + self.shift_ev) * std::f32::consts::LOG10_2;
        let log_y = self.odt.forward_log10(self.rrt.forward_log10(log_x));
        if log_y <= self.log_black {
            return 0.0;
        }
        let nits = (log_y * std::f32::consts::LOG2_10).exp2();
        ((nits - self.black_level) * self.inverse_range).clamp(0.0, 1.0)
    }

    /// Эрмитова кубика от колена (`u_k`, `s_k`) до белого точки (`1`, наклон `0`).
    fn approach_to_white(&self, exposure: f32, white_ev: f32) -> f32 {
        let width = white_ev - self.knee_ev;
        let t = (exposure - self.knee_ev) / width;
        let t2 = t * t;
        let t3 = t2 * t;
        let start_value = 2.0 * t3 - 3.0 * t2 + 1.0;
        let start_slope = t3 - 2.0 * t2 + t;
        let end_value = -2.0 * t3 + 3.0 * t2;
        start_value * self.knee_value + start_slope * width * self.knee_slope + end_value
    }
}
