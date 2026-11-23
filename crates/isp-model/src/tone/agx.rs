//! Основа тона AgX: своя математика по публичному описанию идеи (clean-room, решения 101, 176, 177).
//!
//! Идея AgX из открытых описаний (заметки автора Troy Sobotka, документация Blender 4.x): свет сцены
//! сводится «внутрь» (матрица inset), берётся логарифм вокруг среднего серого, по каналам применяется
//! S-кривая, затем матрица «наружу» (outset) и кривая передачи экрана. Чужой код, чужие таблицы LUT и
//! чужие числа здесь не использованы (у репозиториев AgX нет лицензии, darktable — GPL): формулы ниже
//! выведены заново, числа — стартовые оценки конфига.
//!
//! # Цепочка пикселя (рабочее пространство W = линейный Rec. 2020)
//! ```text
//! w   = вход(c)                       — обрезка нулём или сжатие охвата W
//! v   = M_in · w                      — матрица «внутрь»
//! e   = log2(v / g)                   — ступени относительно серого g = scene_middle_gray / k  (k — подтяжка AE)
//! y   = кривая(e; Δ_sh)               — по каналам, Δ_sh = log2(L / g), L — свой белый точки (решение 177)
//! lin = y^γ                           — линейный свет экрана
//! lin = вернуть_оттенок(v, lin)       — шаг решения 180 ([`super::agx_hue`]); степень 0 — пропуск
//! out = M_out · lin                   — матрица «наружу»
//! ```
//! затем [`AgxBase::to_output`]: `W → цвета выхода`, мягкое сжатие охвата выхода, обрезка 0…1.
//! `v ≤ 0` и NaN дают `y = 0`; NaN в любом канале пикселя — чёрный пиксель (как у сжатия охвата).
//!
//! # Кривая в ступенях `e`
//! Опорная функция `E(u, p) = u / (1 + u^p)^(1/p)`: `E(0) = 0`, `E'(0) = 1`, `E → 1` при `u → ∞`.
//! `y_p = display_middle_gray^(1/γ)` — выход на сером (серый сцены попадает ровно в заданный серый
//! экрана), `s` — наклон на сером (на ступень), `Δ_toe` — глубина чёрного.
//!
//! Плечо (`0 ≤ e < Δ_sh`): `y = y_p + k1·E(s·e/k1, p1)`. Константа `k1` берётся из условия `y(Δ_sh) = 1`.
//! Пусть `r1 = s·Δ_sh/(1 − y_p)`. Из `k1·E(u) = 1 − y_p` при `u = s·Δ_sh/k1` получаем
//! `(1 + u^p1)^(1/p1) = r1`, то есть `u = (r1^p1 − 1)^(1/p1)` и `k1 = s·Δ_sh / (r1^p1 − 1)^(1/p1)`
//! (нужно `r1 > 1`). Подстановка `u^p1 = (e/Δ_sh)^p1 · (r1^p1 − 1)` убирает `k1` (он при `r1 → 1` даёт
//! `∞·0`) и даёт эквивалентную форму, по которой считают CPU и шейдер:
//! ```text
//! y = y_p + s·e / (1 + (e/Δ_sh)^p1 · (r1^p1 − 1))^(1/p1)
//! ```
//! При `e ≥ Δ_sh` — ровно 1. Носок (`−Δ_toe < e < 0`, `t = −e`, `r0 = s·Δ_toe/y_p > 1`):
//! `y = y_p − s·t / (1 + (t/Δ_toe)^p0 · (r0^p0 − 1))^(1/p0)`; при `e ≤ −Δ_toe` — ровно 0.
//! В `e = 0` значение и наклон (`s`) непрерывны с обеих сторон (C1).

use std::sync::Arc;

use super::agx_hue::{HueRestoration, HueRestorationParameters};
use super::agx_settings::{AgxSettings, InvalidAgx};
use super::working_io::WorkingSpaceIo;
use crate::color::matrix::PixelMatrix;
use crate::color::working_space::{inset_matrix, outset_matrix};
use crate::gamut::GamutCompressionParameters;
use crate::image::DisplayGamut;

/// Меньше этого белого точка быть не может (`L = min g'·wb ≥ 1`: усиления виньетирования и баланса ≥ 1),
/// проверка кривой (`r1 > 1`) выполнена именно при нём; шейдер повторяет `max(L, 1)`.
pub const MIN_WHITE_LEVEL: f32 = 1.0;

/// Разрешённая кривая AgX: числа формулы в f32 (CPU и GPU берут одни и те же).
#[derive(Debug, Clone, Copy, PartialEq)]
struct AgxCurve {
    grey: f32,
    gray_output: f32,
    gamma: f32,
    slope: f32,
    toe_power: f32,
    toe_inverse_power: f32,
    toe_depth: f32,
    toe_excess: f32,
    shoulder_power: f32,
    shoulder_inverse_power: f32,
    shoulder_scale: f32,
}

impl AgxCurve {
    fn resolve(settings: &AgxSettings) -> Result<Self, InvalidAgx> {
        settings.validate_ranges()?;
        let grey = f64::from(settings.scene_middle_gray);
        let gamma = f64::from(settings.display_gamma);
        let slope = f64::from(settings.slope);
        let toe_depth = f64::from(settings.toe_depth_ev);
        let gray_output = f64::from(settings.display_middle_gray).powf(1.0 / gamma);
        // Наименьший запас до белого — при L = 1; он положителен, потому что серый после подтяжки < 1 (проверено).
        let headroom = (f64::from(MIN_WHITE_LEVEL) / grey).log2();
        let shoulder_scale = slope / (1.0 - gray_output);
        let shoulder_ratio = shoulder_scale * headroom;
        if shoulder_ratio.is_nan() || shoulder_ratio <= 1.0 {
            return Err(InvalidAgx::ShoulderTooShallow {
                ratio: shoulder_ratio as f32,
                slope: settings.slope,
                headroom_ev: headroom as f32,
                minimum_slope: ((1.0 - gray_output) / headroom) as f32,
            });
        }
        let toe_ratio = slope * toe_depth / gray_output;
        if toe_ratio.is_nan() || toe_ratio <= 1.0 {
            return Err(InvalidAgx::ToeTooShallow {
                ratio: toe_ratio as f32,
                slope: settings.slope,
                toe_depth_ev: settings.toe_depth_ev,
                minimum_depth_ev: (gray_output / slope) as f32,
            });
        }
        let toe_power = f64::from(settings.toe_power);
        let shoulder_power = f64::from(settings.shoulder_power);
        Ok(Self {
            grey: settings.scene_middle_gray,
            gray_output: gray_output as f32,
            gamma: settings.display_gamma,
            slope: settings.slope,
            toe_power: settings.toe_power,
            toe_inverse_power: (1.0 / toe_power) as f32,
            toe_depth: settings.toe_depth_ev,
            toe_excess: (toe_ratio.powf(toe_power) - 1.0) as f32,
            shoulder_power: settings.shoulder_power,
            shoulder_inverse_power: (1.0 / shoulder_power) as f32,
            shoulder_scale: shoulder_scale as f32,
        })
    }

    /// Выход кривой `y ∈ [0, 1]` для света одного канала `x` и белого точки `white` (формула модуля).
    fn map_channel(&self, x: f32, white: f32) -> f32 {
        if x.is_nan() || x <= 0.0 {
            return 0.0;
        }
        let e = (x / self.grey).log2();
        let delta = (white.max(MIN_WHITE_LEVEL) / self.grey).log2();
        if e >= delta {
            return 1.0;
        }
        let y = if e >= 0.0 {
            let ratio = self.shoulder_scale * delta;
            let excess = ratio.powf(self.shoulder_power) - 1.0;
            let blend = (e / delta).powf(self.shoulder_power) * excess;
            self.gray_output + self.slope * e / (1.0 + blend).powf(self.shoulder_inverse_power)
        } else {
            let depth = -e;
            if depth >= self.toe_depth {
                return 0.0;
            }
            let blend = (depth / self.toe_depth).powf(self.toe_power) * self.toe_excess;
            self.gray_output - self.slope * depth / (1.0 + blend).powf(self.toe_inverse_power)
        };
        y.clamp(0.0, 1.0)
    }
}

/// Числа основы AgX для шейдера: ровно те f32, что подставляет [`AgxBase::map_scene_pixel`] и
/// [`AgxBase::to_output`]. Только чтение: собрать [`AgxBase`] из них нельзя (единственный вход —
/// [`AgxBase::resolve`]), поэтому несогласованные числа не построить.
///
/// Матрицы — по строкам; порядок применения указан в доке модуля [`super::agx`]. Кривая:
/// `grey`, `gray_output`, `gamma`, `slope`, `toe_*`, `shoulder_*` — по формулам модуля;
/// `shoulder_scale = s/(1 − y_p)` (`r1 = shoulder_scale·Δ_sh` считается на пиксель),
/// `toe_excess = r0^p0 − 1`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct AgxParameters {
    pub xyz_d50_to_working: [[f32; 3]; 3],
    pub inset: [[f32; 3]; 3],
    pub outset: [[f32; 3]; 3],
    pub working_to_output: [[f32; 3]; 3],
    /// Средний серый сцены с подтяжкой AE: `scene_middle_gray / k`.
    pub grey: f32,
    /// `y_p`: выход кривой на сером.
    pub gray_output: f32,
    pub gamma: f32,
    pub slope: f32,
    pub toe_power: f32,
    pub toe_inverse_power: f32,
    pub toe_depth: f32,
    pub toe_excess: f32,
    pub shoulder_power: f32,
    pub shoulder_inverse_power: f32,
    pub shoulder_scale: f32,
    /// Сжатие охвата на входе: `None` — обрезка нулём.
    pub input_compression: Option<GamutCompressionParameters>,
    pub output_compression: GamutCompressionParameters,
    /// Возврат оттенка после кривой (решение 180): `None` — шаг пропущен (степень 0).
    pub hue_restoration: Option<HueRestorationParameters>,
}

/// Разрешённая основа AgX на кадр: матрицы, кривая и сжатия охвата.
#[derive(Debug, Clone, PartialEq)]
pub struct AgxBase {
    /// Общий шаг входа и выхода рабочего пространства (один на все модели основы).
    io: WorkingSpaceIo,
    inset: PixelMatrix,
    outset: PixelMatrix,
    curve: AgxCurve,
    hue: Option<HueRestoration>,
}

impl AgxBase {
    /// Разрешает настройки (уже с подтяжкой AE, см. [`AgxSettings::with_exposure_gain`]) для гаммы выхода.
    /// Матрицы считаются в f64 и сужаются до f32 один раз.
    pub fn resolve(settings: &AgxSettings, output_gamut: DisplayGamut) -> Result<Arc<Self>, InvalidAgx> {
        let curve = AgxCurve::resolve(settings)?;
        Ok(Arc::new(Self {
            io: WorkingSpaceIo::resolve(&settings.working_io(), output_gamut)?,
            inset: inset_matrix(&settings.inset)?.to_f32(),
            outset: outset_matrix(&settings.outset)?.to_f32(),
            curve,
            hue: HueRestoration::resolve(settings)?,
        }))
    }

    /// Гамма выхода, под которую посчитана матрица `W → выход`.
    pub fn output_gamut(&self) -> DisplayGamut {
        self.io.output_gamut()
    }

    /// Числа формулы для GPU (см. [`AgxParameters`]).
    pub fn parameters(&self) -> AgxParameters {
        let io = self.io.parameters();
        AgxParameters {
            xyz_d50_to_working: io.xyz_d50_to_working,
            inset: self.inset.rows,
            outset: self.outset.rows,
            working_to_output: io.working_to_output,
            grey: self.curve.grey,
            gray_output: self.curve.gray_output,
            gamma: self.curve.gamma,
            slope: self.curve.slope,
            toe_power: self.curve.toe_power,
            toe_inverse_power: self.curve.toe_inverse_power,
            toe_depth: self.curve.toe_depth,
            toe_excess: self.curve.toe_excess,
            shoulder_power: self.curve.shoulder_power,
            shoulder_inverse_power: self.curve.shoulder_inverse_power,
            shoulder_scale: self.curve.shoulder_scale,
            input_compression: io.input_compression,
            output_compression: io.output_compression,
            hue_restoration: self.hue.map(|hue| hue.parameters()),
        }
    }

    /// `XYZ D50 → W` одного пикселя.
    pub fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        self.io.xyz_d50_to_working(xyz_d50)
    }

    /// Выход кривой по каналу (для тестов формулы и измерений): `y ∈ [0, 1]` до `y^γ`.
    pub fn curve_channel(&self, light: f32, white: f32) -> f32 {
        self.curve.map_channel(light, white)
    }

    /// Основа AgX для пикселя сцены в W со своим белым `white` → линейный свет экрана в W (формула модуля).
    pub fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        if scene.iter().any(|channel| channel.is_nan()) {
            return [0.0; 3];
        }
        let entering = self.apply_input(scene);
        let inset = self.inset.apply(entering);
        let curved = inset.map(|channel| self.curve.map_channel(channel, white).powf(self.curve.gamma));
        // Возврат оттенка (решение 180): оттенок цвета до кривой (после inset) цвету после неё; степень 0 — пропуск.
        let shaped = match &self.hue {
            Some(hue) => hue.restore(inset, curved),
            None => curved,
        };
        self.outset.apply(shaped)
    }

    /// Вход в W: обрезка нулём или мягкое сжатие охвата (общий шаг [`WorkingSpaceIo`]).
    pub fn apply_input(&self, scene: [f32; 3]) -> [f32; 3] {
        self.io.apply_input(scene)
    }

    /// Последний шаг: `W → цвета выхода`, мягкое сжатие охвата выхода и обрезка 0…1 (общий шаг).
    pub fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        self.io.to_output(display_working)
    }
}

#[cfg(test)]
#[path = "agx_tests.rs"]
mod tests;
