//! Основа тона filmic: своя математика по публичному описанию идеи (clean-room, решения 47, 176, 177, 182).
//!
//! # Что взято и что своё
//! **Взято (идеи и определения из руководства darktable «filmic rgb» и статьи автора модуля Aurélien Pierre
//! «Filmic, darktable and the quest of the HDR tone-mapping», 2018):**
//! * свет сцены кодируется логарифмом между «чёрной» и «белой» относительной экспозицией вокруг среднего серого;
//! * вокруг серого — участок «латитюды»: прямая в логарифмических осях с заданным контрастом (наклоном);
//! * к чёрному и к белому прямая плавно переходит в загнутые концы (в нашей терминологии — носок и плечо);
//! * цветность сохраняется: кривая применяется к НОРМЕ пикселя, а RGB масштабируется отношением «норма после /
//!   норма до»; варианты нормы — максимум каналов, яркость, «степенная норма» `(R³+G³+B³)/(R²+G²+B²)` (её
//!   определение дано в руководстве как факт, формула общеизвестна) и «без нормы» (по каналам);
//! * крайние света и тени обесцвечиваются, чтобы цвет уходил к белому/чёрному, а не упирался в границу охвата.
//!
//! **Своё (выведено заново, код darktable не открывался и не копировался):** вся математика кривой — форма носка
//! и плеча (опорная функция `E`, см. ниже), условия разрешимости, привязка серого и белого точки; способ
//! обесцвечивания (смесь в OKLab к белому с весом от выхода кривой — в оригинале это гауссова маска по
//! яркости); вписывание в охват выхода; выбор нормы замером.
//!
//! **Лицензия источника:** руководство darktable (репозиторий dtdocs) выпущено под GPL-3.0: в `content/_index.md`
//! сказано «This documentation is released under the GPL 3.0 license». Мы не переносим из него текст и не
//! копируем формулы реализации, только описание идеи и название параметров. Код модуля darktable — GPL-3.0, не
//! использовался. Отклонено: игровая кривая Hable/Uncharted 2 — у неё нет логарифмического управления
//! диапазоном и «своего белого» точки (решение 177): белый фиксирован константой.
//!
//! # Цепочка пикселя (рабочее пространство W = линейный Rec. 2020)
//! ```text
//! v   = вход(c)                              — обрезка нулём или сжатие охвата W (тот же шаг, что у AgX)
//! N   = норма(v)                             — по [`FilmicNorm`]; для PerChannel нормы нет
//! e   = log2(N / g)                          — ступени относительно серого g = scene_middle_gray / k (k — подтяжка AE)
//! y   = кривая(e; Δ_w)                       — Δ_w = log2(L / g), L — свой белый точки (решение 177)
//! lin = y^γ                                  — линейный свет экрана
//! d   = smootherstep((y − o)/(1 − o))        — обесцвечивание в светах, o = desaturation_onset
//! c   = lin · v/N                           — цвет сохранён (отношения каналов те же)
//! out = OKLab-смесь(c → белый lin; d)        — обесцвечивание без смены оттенка; d = 1 → ровно белый
//! out = out / max(1, max (W→выход)·out)      — вписать в охват выхода без смены цветности
//! ```
//! У `PerChannel`: `out_c = кривая(v_c)^γ` для каждого канала. Затем общий шаг [`FilmicBase::to_output`]
//! (он же [`super::agx::AgxBase::to_output`]): `W → цвета выхода`, мягкое сжатие охвата, обрезка 0…1.
//! NaN в любом канале, `N ≤ 0`, `v ≤ 0` дают чёрный пиксель.
//!
//! # Кривая `y(e)` в ступенях `e` (все числа в `[0, 1]`, `y` — до степени экрана `γ`)
//! Пусть `y_p = display_middle_gray^(1/γ)` (серый сцены попадает ровно в заданный серый экрана), `s` — наклон,
//! `Δ_b` — глубина чёрного, `Δ_w` — запас до белого. Линейный участок: `y = y_p + s·e` на `−a ≤ e ≤ b`, где
//! ```text
//! a = f_t·y_p/s            y_lo = (1 − f_t)·y_p                  (f_t — toe_linear_share)
//! b = f_h·(1 − y_p)/s      y_hi = y_p + f_h·(1 − y_p),  h = 1 − y_hi    (f_h — shoulder_linear_share)
//! ```
//! Носок (`e < −a`, `t = −a − e`, ширина `w_t = Δ_b − a`, `r0 = s·w_t/y_lo`):
//! `y = y_lo − s·t / (1 + (t/w_t)^p0·(r0^p0 − 1))^(1/p0)`; при `t ≥ w_t` — ровно 0.
//! Плечо (`e > b`, `t = e − b`, ширина `w_s = Δ_w − b`, `r1 = s·w_s/h`):
//! `y = y_hi + s·t / (1 + (t/w_s)^p1·(r1^p1 − 1))^(1/p1)`; при `e ≥ Δ_w` — ровно 1.
//!
//! ## Вывод
//! Опорная функция `E(u, p) = u/(1 + u^p)^(1/p)`: `E(0) = 0`, `E'(0) = 1`, `E'' (0) = 0` при `p > 1`, `E` строго
//! возрастает. Носок — `y = y_lo − k·E(s·t/k, p0)`: наклон в стыке `−s` по `t`, то есть `+s` по `e` (C1), а при
//! `p0 > 1` и вторая производная в стыке равна нулю, как у прямой (C2). Константа `k` выбирается из условия
//! `y(w_t) = 0`: `k·E(u) = y_lo` при `u = s·w_t/k`, откуда `(1 + u^p0)^(1/p0) = s·w_t/y_lo = r0`, значит
//! `u = (r0^p0 − 1)^(1/p0)`, `k = s·w_t/u`. Подстановка `u^p0 = (t/w_t)^p0·(r0^p0 − 1)` убирает `k` и даёт форму
//! выше (так считают CPU и шейдер; плечо — зеркально, с `h` вместо `y_lo`).
//!
//! ## Условия разрешимости
//! Решение существует тогда и только тогда, когда `r0 > 1` и `r1 > 1` (выпуклость: прямая, продолженная до
//! чёрного/белого, должна уходить за цель). Подставив `a`, `b`, получаем условия, НЕ зависящие от долей
//! `f_t`, `f_h`, степеней и нормы:
//! ```text
//! носок:  s·Δ_b > y_p           плечо:  s·Δ_w > 1 − y_p     (худший случай Δ_w при L = 1)
//! ```
//! Белый точки `L ≥ 1` только увеличивает `Δ_w`, поэтому проверка при `L = 1` достаточна ([`MIN_WHITE_LEVEL`]).
//! При невозможных параметрах — различимые [`InvalidFilmic::ToeTooShallow`] / [`InvalidFilmic::ShoulderTooShallow`]
//! с подсказкой минимального наклона.
//!
//! ## Свойства
//! Серый сцены `g` → `y_p` → ровно заданный серый экрана (не зависит от `L`); белый точки `e = Δ_w` → ровно 1;
//! монотонность; значение и наклон непрерывны на обоих стыках. На концах у чёрного и белого наклон `y` не нулевой,
//! но после степени `γ > 1` свет экрана у чёрного приходит в 0 с нулевым наклоном.
//!
//! # Заметка о коде общего шага
//! Вход в W (сжатие охвата) и выход (`W → цвета выхода`, сжатие, обрезка) — общий для всех моделей
//! [`super::working_io::WorkingSpaceIo`]; здесь он лежит в поле `io`.

use std::sync::Arc;

#[path = "filmic_settings.rs"]
pub mod settings;

pub use self::settings::{FilmicNorm, FilmicSettings, InvalidFilmic};
use super::agx::MIN_WHITE_LEVEL;
use super::working_io::{WorkingIoParameters, WorkingSpaceIo};
use crate::color::oklab_working::OklabWorkspace;
use crate::color::working_space::WORKING_PRIMARIES;
use crate::gamut::GamutCompressionParameters;
use crate::image::DisplayGamut;

/// Свет сцены выше этого значения (в единицах нормированного света) обрезается: защита от `inf/inf` при
/// делении `v/N` и от переполнения куба в степенной норме. Реальный свет не бывает больше `L ≲ 10²`.
const MAX_SCENE_LIGHT: f32 = 1.0e20;

/// Допуск на округление строк матрицы `W → выход` (каждая строка суммируется в 1 с точностью f32): белый и серый,
/// у которых наибольший канал отличается от 1 лишь на округление, вписыванием не трогаются.
const FIT_TOLERANCE: f32 = 1.0e-5;

/// Разрешённая кривая filmic: числа формулы в f32 (CPU и GPU берут одни и те же).
#[derive(Debug, Clone, Copy, PartialEq)]
struct FilmicCurve {
    grey: f32,
    gray_output: f32,
    gamma: f32,
    slope: f32,
    /// `a`: длина линейного участка вниз от серого, ступеней.
    linear_below: f32,
    /// `y_lo`: выход на нижнем краю линейного участка.
    toe_value: f32,
    /// `w_t = Δ_b − a`.
    toe_width: f32,
    toe_power: f32,
    toe_inverse_power: f32,
    /// `r0^p0 − 1`.
    toe_excess: f32,
    /// `b`: длина линейного участка вверх от серого, ступеней.
    linear_above: f32,
    /// `y_hi`: выход на верхнем краю линейного участка.
    shoulder_value: f32,
    /// `h = 1 − y_hi`.
    shoulder_headroom: f32,
    shoulder_power: f32,
    shoulder_inverse_power: f32,
    desaturation_onset: f32,
}

impl FilmicCurve {
    fn resolve(settings: &FilmicSettings) -> Result<Self, InvalidFilmic> {
        settings.validate_ranges()?;
        let grey = f64::from(settings.scene_middle_gray);
        let gamma = f64::from(settings.display_gamma);
        let slope = f64::from(settings.slope);
        let black_depth = f64::from(settings.black_depth_ev);
        let gray_output = f64::from(settings.display_middle_gray).powf(1.0 / gamma);

        // Плечо: худший запас до белого — при L = 1 (он положителен: серый после подтяжки < 1).
        let white_headroom = (f64::from(MIN_WHITE_LEVEL) / grey).log2();
        let shoulder_required = 1.0 - gray_output;
        let shoulder_rise = slope * white_headroom;
        if shoulder_rise.is_nan() || shoulder_rise <= shoulder_required {
            return Err(InvalidFilmic::ShoulderTooShallow {
                slope: settings.slope,
                headroom_ev: white_headroom as f32,
                rise: shoulder_rise as f32,
                required: shoulder_required as f32,
                minimum_slope: (shoulder_required / white_headroom) as f32,
            });
        }
        // Носок: условие `s·Δ_b > y_p`.
        let toe_rise = slope * black_depth;
        if toe_rise.is_nan() || toe_rise <= gray_output {
            return Err(InvalidFilmic::ToeTooShallow {
                slope: settings.slope,
                black_depth_ev: settings.black_depth_ev,
                rise: toe_rise as f32,
                required: gray_output as f32,
                minimum_slope: (gray_output / black_depth) as f32,
                minimum_depth_ev: (gray_output / slope) as f32,
            });
        }

        let toe_share = f64::from(settings.toe_linear_share);
        let linear_below = toe_share * gray_output / slope;
        let toe_value = (1.0 - toe_share) * gray_output;
        let toe_width = black_depth - linear_below;
        let toe_ratio = slope * toe_width / toe_value;
        let toe_power = f64::from(settings.toe_power);

        let shoulder_share = f64::from(settings.shoulder_linear_share);
        let linear_above = shoulder_share * shoulder_required / slope;
        let shoulder_value = gray_output + shoulder_share * shoulder_required;
        let shoulder_headroom = (1.0 - shoulder_share) * shoulder_required;
        let shoulder_power = f64::from(settings.shoulder_power);

        Ok(Self {
            grey: settings.scene_middle_gray,
            gray_output: gray_output as f32,
            gamma: settings.display_gamma,
            slope: settings.slope,
            linear_below: linear_below as f32,
            toe_value: toe_value as f32,
            toe_width: toe_width as f32,
            toe_power: settings.toe_power,
            toe_inverse_power: (1.0 / toe_power) as f32,
            toe_excess: (toe_ratio.powf(toe_power) - 1.0) as f32,
            linear_above: linear_above as f32,
            shoulder_value: shoulder_value as f32,
            shoulder_headroom: shoulder_headroom as f32,
            shoulder_power: settings.shoulder_power,
            shoulder_inverse_power: (1.0 / shoulder_power) as f32,
            desaturation_onset: settings.desaturation_onset,
        })
    }

    /// Выход кривой `y ∈ [0, 1]` для света (нормы) `x` и белого точки `white` (формула модуля).
    fn map_light(&self, x: f32, white: f32) -> f32 {
        if x.is_nan() || x <= 0.0 {
            return 0.0;
        }
        let e = (x / self.grey).log2();
        let delta = (white.max(MIN_WHITE_LEVEL) / self.grey).log2();
        if e >= delta {
            return 1.0;
        }
        let y = if e > self.linear_above {
            let rise = e - self.linear_above;
            let width = delta - self.linear_above;
            let ratio = self.slope * width / self.shoulder_headroom;
            let excess = ratio.powf(self.shoulder_power) - 1.0;
            let blend = (rise / width).powf(self.shoulder_power) * excess;
            self.shoulder_value + self.slope * rise / (1.0 + blend).powf(self.shoulder_inverse_power)
        } else if e >= -self.linear_below {
            self.gray_output + self.slope * e
        } else {
            let depth = -self.linear_below - e;
            if depth >= self.toe_width {
                return 0.0;
            }
            let blend = (depth / self.toe_width).powf(self.toe_power) * self.toe_excess;
            self.toe_value - self.slope * depth / (1.0 + blend).powf(self.toe_inverse_power)
        };
        y.clamp(0.0, 1.0)
    }

    /// Вес обесцвечивания в светах `d ∈ [0, 1]` по выходу кривой `y` (гладкая «smootherstep», C2).
    fn desaturation_weight(&self, y: f32) -> f32 {
        let t = ((y - self.desaturation_onset) / (1.0 - self.desaturation_onset)).clamp(0.0, 1.0);
        t * t * t * (t * (6.0 * t - 15.0) + 10.0)
    }
}

/// Числа основы filmic для шейдера: ровно те f32, что подставляет [`FilmicBase::map_scene_pixel`] и
/// [`FilmicBase::to_output`]. Только чтение: собрать [`FilmicBase`] из них нельзя (единственный вход —
/// [`FilmicBase::resolve`]).
///
/// Порядок формул — в доке модуля [`crate::tone::filmic`]. `luminance_row` нужна только норме `Luminance`.
/// Общий шаг входа/выхода — те же числа, что у [`WorkingIoParameters`] (матрицы и сжатия охвата).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct FilmicParameters {
    pub xyz_d50_to_working: [[f32; 3]; 3],
    pub working_to_output: [[f32; 3]; 3],
    /// Сжатие охвата на входе: `None` — обрезка нулём.
    pub input_compression: Option<GamutCompressionParameters>,
    pub output_compression: GamutCompressionParameters,
    /// Норма пикселя (порядок вариантов [`FilmicNorm`]: `PerChannel`, `MaxRgb`, `Luminance`, `PowerNorm`).
    pub norm: FilmicNorm,
    /// Строка яркости `Y` рабочего пространства (сумма 1).
    pub luminance_row: [f32; 3],
    /// Средний серый сцены с подтяжкой AE: `scene_middle_gray / k`.
    pub grey: f32,
    /// `y_p`: выход кривой на сером.
    pub gray_output: f32,
    pub gamma: f32,
    pub slope: f32,
    /// `a`: длина линейного участка вниз, ступеней.
    pub linear_below: f32,
    /// `y_lo`.
    pub toe_value: f32,
    /// `w_t = Δ_b − a`.
    pub toe_width: f32,
    pub toe_power: f32,
    pub toe_inverse_power: f32,
    /// `r0^p0 − 1`.
    pub toe_excess: f32,
    /// `b`: длина линейного участка вверх, ступеней.
    pub linear_above: f32,
    /// `y_hi`.
    pub shoulder_value: f32,
    /// `h = 1 − y_hi`; `r1 = slope·(Δ_w − b)/h` считается на пиксель (`Δ_w` зависит от белого точки).
    pub shoulder_headroom: f32,
    pub shoulder_power: f32,
    pub shoulder_inverse_power: f32,
    /// Порог обесцвечивания в светах `o`.
    pub desaturation_onset: f32,
    /// Матрицы OKLab рабочего пространства (обесцвечивание в светах).
    pub oklab: OklabWorkspace,
}

/// Разрешённая основа filmic на кадр: кривая, норма и общий шаг входа/выхода.
#[derive(Debug, Clone, PartialEq)]
pub struct FilmicBase {
    curve: FilmicCurve,
    norm: FilmicNorm,
    luminance_row: [f32; 3],
    /// Матрицы OKLab для обесцвечивания в светах.
    oklab: OklabWorkspace,
    /// Общий шаг рабочего пространства: вход в W, `XYZ D50 → W`, `W → выход`, сжатие охвата выхода.
    io: WorkingSpaceIo,
}

impl FilmicBase {
    /// Разрешает настройки (уже с подтяжкой AE, см. [`FilmicSettings::with_exposure_gain`]) для гаммы выхода.
    pub fn resolve(settings: &FilmicSettings, output_gamut: DisplayGamut) -> Result<Arc<Self>, InvalidFilmic> {
        let curve = FilmicCurve::resolve(settings)?;
        let io = WorkingSpaceIo::resolve(&settings.working_io(), output_gamut)?;
        let luminance_row = luminance_row()?;
        Ok(Arc::new(Self {
            curve,
            norm: settings.norm,
            luminance_row,
            oklab: OklabWorkspace::new(&WORKING_PRIMARIES)?,
            io,
        }))
    }

    /// Гамма выхода, под которую посчитана матрица `W → выход`.
    pub fn output_gamut(&self) -> DisplayGamut {
        self.io.output_gamut()
    }

    /// Числа формулы для GPU (см. [`FilmicParameters`]).
    pub fn parameters(&self) -> FilmicParameters {
        let io: WorkingIoParameters = self.io.parameters();
        let curve = &self.curve;
        FilmicParameters {
            xyz_d50_to_working: io.xyz_d50_to_working,
            working_to_output: io.working_to_output,
            input_compression: io.input_compression,
            output_compression: io.output_compression,
            norm: self.norm,
            luminance_row: self.luminance_row,
            grey: curve.grey,
            gray_output: curve.gray_output,
            gamma: curve.gamma,
            slope: curve.slope,
            linear_below: curve.linear_below,
            toe_value: curve.toe_value,
            toe_width: curve.toe_width,
            toe_power: curve.toe_power,
            toe_inverse_power: curve.toe_inverse_power,
            toe_excess: curve.toe_excess,
            linear_above: curve.linear_above,
            shoulder_value: curve.shoulder_value,
            shoulder_headroom: curve.shoulder_headroom,
            shoulder_power: curve.shoulder_power,
            shoulder_inverse_power: curve.shoulder_inverse_power,
            desaturation_onset: curve.desaturation_onset,
            oklab: self.oklab,
        }
    }

    /// `XYZ D50 → W` одного пикселя.
    pub fn xyz_d50_to_working(&self, xyz_d50: [f32; 3]) -> [f32; 3] {
        self.io.xyz_d50_to_working(xyz_d50)
    }

    /// Выход кривой `y ∈ [0, 1]` для света `light` (нормы пикселя или канала) и белого `white`, до `y^γ`
    /// (для тестов формулы и измерений).
    pub fn curve_value(&self, light: f32, white: f32) -> f32 {
        self.curve.map_light(light, white)
    }

    /// Длины линейного участка `(вниз, вверх)` от серого в ступенях: `(a, b)`.
    pub fn linear_extent_ev(&self) -> (f32, f32) {
        (self.curve.linear_below, self.curve.linear_above)
    }

    /// Основа filmic для пикселя сцены в W со своим белым `white` → линейный свет экрана в W (формула модуля).
    pub fn map_scene_pixel(&self, scene: [f32; 3], white: f32) -> [f32; 3] {
        if scene.iter().any(|channel| channel.is_nan()) {
            return [0.0; 3];
        }
        let light = self
            .io
            .apply_input(scene)
            .map(|channel| channel.clamp(0.0, MAX_SCENE_LIGHT));
        let curve = &self.curve;
        if self.norm == FilmicNorm::PerChannel {
            return light.map(|channel| curve.map_light(channel, white).powf(curve.gamma));
        }
        let norm = self.norm_of(light);
        if norm <= 0.0 {
            return [0.0; 3];
        }
        let curved = curve.map_light(norm, white);
        let display = curved.powf(curve.gamma);
        let desaturation = curve.desaturation_weight(curved);
        let coloured = light.map(|channel| display * (channel / norm));
        self.fit_to_output_gamut(self.desaturate_towards_white(coloured, display, desaturation))
    }

    /// Обесцвечивание в светах без смены оттенка: смесь в OKLab между цветом и белым той же светлоты `display`
    /// (`L, a, b` → `(1 − d)·(L, a, b) + d·(L_белого, 0, 0)`). Оттенок OKLab при этом точно сохраняется — смесь
    /// в линейном свете сдвигала его на 5–9° (оранжевый уходил в жёлтое). На `d = 0` — цвет как есть, на
    /// `d = 1` — ровно `(display, display, display)` (без округления матриц OKLab).
    fn desaturate_towards_white(&self, coloured: [f32; 3], display: f32, weight: f32) -> [f32; 3] {
        if weight <= 0.0 {
            return coloured;
        }
        if weight >= 1.0 {
            return [display; 3];
        }
        let oklab = &self.oklab;
        let [lightness, a, b] = oklab.to_lab(coloured);
        let [white_lightness, _, _] = oklab.to_lab([display; 3]);
        let keep = 1.0 - weight;
        oklab.to_rgb([keep * lightness + weight * white_lightness, keep * a, keep * b])
    }

    /// Вписывает цвет в охват ВЫХОДА без смены цветности: если после `W → выход` какой-то канал больше 1,
    /// весь цвет делится на наибольший канал. Иначе насыщенный яркий цвет (оранжевый при норме максимума,
    /// синий при норме яркости) упирался бы в обрезку одного канала при выводе, и оттенок уходил на
    /// 5–10°. Серый и белый проходят без изменения (допуск [`FIT_TOLERANCE`] на округление строк матрицы).
    fn fit_to_output_gamut(&self, display_working: [f32; 3]) -> [f32; 3] {
        let output = self.io.working_to_output_colors(display_working);
        let largest = output.iter().copied().fold(1.0_f32, f32::max);
        if largest <= 1.0 + FIT_TOLERANCE {
            return display_working;
        }
        display_working.map(|channel| channel / largest)
    }

    /// Вход в W: обрезка нулём или мягкое сжатие охвата (общий шаг).
    pub fn apply_input(&self, scene: [f32; 3]) -> [f32; 3] {
        self.io.apply_input(scene)
    }

    /// Последний шаг: `W → цвета выхода`, мягкое сжатие охвата выхода и обрезка 0…1 (общий шаг).
    pub fn to_output(&self, display_working: [f32; 3]) -> [f32; 3] {
        self.io.to_output(display_working)
    }

    /// Норма пикселя `N(v)` для цветных норм; `v ≥ 0`.
    fn norm_of(&self, light: [f32; 3]) -> f32 {
        let largest = light[0].max(light[1]).max(light[2]);
        match self.norm {
            // PerChannel сюда не попадает (обработан выше); норма максимума — безопасный ответ.
            FilmicNorm::PerChannel | FilmicNorm::MaxRgb => largest,
            FilmicNorm::Luminance => {
                let row = self.luminance_row;
                row[0] * light[0] + row[1] * light[1] + row[2] * light[2]
            }
            FilmicNorm::PowerNorm => {
                if largest <= 0.0 {
                    return 0.0;
                }
                // (R³+G³+B³)/(R²+G²+B²) = m·Σu³/Σu² при u = c/m, m = max: нет переполнения куба.
                let unit = light.map(|channel| channel / largest);
                let squares: f32 = unit.iter().map(|u| u * u).sum();
                let cubes: f32 = unit.iter().map(|u| u * u * u).sum();
                largest * cubes / squares
            }
        }
    }
}

impl FilmicSettings {
    /// Полная проверка настроек: диапазоны чисел, условия разрешимости кривой (`s·Δ_b > y_p`, `s·Δ_w > 1 − y_p`
    /// при белом точки `L = 1`) и сжатия охвата. Ошибка называет поле или нарушенное условие.
    pub fn validate(&self) -> Result<(), InvalidFilmic> {
        FilmicBase::resolve(self, DisplayGamut::Srgb).map(|_| ())
    }
}

/// Строка яркости `Y` Rec. 2020, нормированная на сумму 1 (серый `(g, g, g)` даёт норму `g`).
fn luminance_row() -> Result<[f32; 3], InvalidFilmic> {
    let row = WORKING_PRIMARIES.rgb_to_xyz()?.rows[1];
    let sum: f64 = row.iter().sum();
    Ok(row.map(|weight| (weight / sum) as f32))
}

#[cfg(test)]
#[path = "filmic_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "filmic_settings_tests.rs"]
mod settings_tests;

#[cfg(test)]
#[path = "filmic_color_tests.rs"]
mod color_tests;
