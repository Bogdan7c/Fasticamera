//! Тон вида: контраст, средние тона, тени, света — отдельная стадия ПОСЛЕ основы тона (решения 236, 244).
//!
//! # Зачем отдельно от основы
//! Раньше эти ручки меняли саму кривую основы (AgX, ACES, filmic, прежняя). У каждой кривой свой запас: у AgX и
//! filmic контраст в минус и света почти не двигались, у прежней теней и светов не было вовсе, у ACES кривая на
//! сильной ручке переставала строиться. Стадия работает над светом экрана, который основа уже отдала, поэтому
//! сила ручки одинакова на всех основах, а «кривая не строится» невозможно по построению.
//! Цена: основа уже сжала самые яркие места — выбитое стадия не вернёт, она перераспределяет то, что осталось.
//!
//! # Кривая одного канала
//! Считается в координате `t = √x` (x — линейный свет экрана `0…1`): она близка к тому, как глаз делит яркость, и
//! дешевле кубического корня. Серый экрана `Y_g` — точка `q = √Y_g`; она делит ось на тени (`u = t/q`) и света
//! (`v = (1 − t)/(1 − q)`, расстояние от белого). Обе половины — дроби без степеней и логарифмов:
//! ```text
//! зона:      Z(u, m) = (u² + m·u·(1−u)) / (1 + (m−1)·u·(1−u))     — Z(0)=0, Z(1)=1, Z'(0)=m, Z'(1)=1
//! контраст:  K(u, a) = u / (u + a·(1−u))                          — K(0)=0, K(1)=1, K'(1)=a, K'(0)=1/a
//! тени:      y = q·K(Z(u, m_тени), a)                             — t ≤ q
//! света:     y = 1 − (1−q)·K(Z(v, m_света), a)                    — t > q
//! средние:   T(y, s) = s·y / (1 + (s−1)·y)                        — T(0)=0, T(1)=1, T'(0)=s, T'(1)=1/s
//! x' = T(y, s)²
//! ```
//! `Z` — рационально-квадратичный отрезок с заданными наклонами на концах (Gregory и Delbourgo, 1982), `K` и `T` —
//! «gain» и «bias» Шлика (Schlick, 1994; с произвольной точкой перегиба — Barron, 2020). Своя реализация по формулам.
//!
//! # Что гарантировано формой (и закреплено тестами)
//! * Чёрное остаётся чёрным, белое — белым: все дроби держат концы отрезка.
//! * Кривая растёт при ЛЮБЫХ положительных `m, a, s` (у каждой дроби производная положительна), композиция тоже.
//!   Поэтому у ручек нет сочетаний, на которых тон «переворачивается», — проверять углы при старте не нужно.
//! * Серый экрана не двигают тени, света и контраст: на стыке половин наклоны обеих равны `a` (C1). Двигает его
//!   только ручка средних тонов — она для этого и есть.
//! * Наклоны на концах конечны: чёрное не «взрывается» бесконечным усилением шума, как у степенной кривой.
//!
//! # Цвет пикселя
//! Кривая применяется к наибольшему и наименьшему каналу. Дальше две версии, обе держат каналы внутри `0…1`:
//! * **растянутая** — средний канал пропорционально между новыми краями (оттенок HSV сохраняется, насыщенность
//!   тянется за контрастом; так устроена тональная кривая DNG SDK);
//! * **подобная** — все каналы умножены на одно число (цвет не меняется вовсе, только свет).
//!
//! Итог — смесь с долей `color_follow` растянутой (решение 243: «наполовину»). Замер кандидатов — `results/14l.md`.
//!
//! # Вне `0…1`
//! Основа отдаёт и чуть отрицательные каналы, и чуть больше единицы (AgX: −0,008…1,067). Кривая там — тождество:
//! непрерывна на стыке, а усиление чёрного (до тысяч на краю шкал) не раздувает то, чего нет в охвате.

use look_contract::LookSettings;
use serde::{Deserialize, Serialize};

use super::model::InvalidLookModel;
use super::range::KnobRange;

/// Наибольший канал, ниже которого пиксель — чёрный: отношение «стало / было» там не имеет смысла в f32. Даже
/// усиленный краями всех ручек, такой свет на порядки темнее одной ступени 10-битного экрана.
const BLACK_FLOOR: f32 = 1e-10;
/// Пределы серого экрана: обеим половинам кривой нужна ширина.
const PIVOT_GRAY_LIMITS: (f32, f32) = (0.01, 0.8);
/// Наибольшая сила ручки тона, `log2` наклона: дальше дроби теряют точность f32 у края отрезка.
const MAX_EDGE_LOG2: f32 = 6.0;

/// Сила ручек тона на краях (конфиг `look_model.tone`). Края выбраны владельцем в 14l по снимкам: «заведомо
/// перебор, как в редакторах» — рабочая зона в середине шкалы (решение 242, мерка — RawTherapee 5.13).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ToneShapeSettings {
    /// Контраст: `log2` наклона кривой на сером (`a = 2^величина`). 2,32 — наклон ×5, ≈ ±50 кодов экрана из 255.
    pub contrast: KnobRange,
    /// Средние тона: `log2` наклона `s` у чёрного. 3 — ×8, ≈ ±122 кода.
    pub midtones: KnobRange,
    /// Тени: `log2` наклона `m` у чёрного (`+` светлее). Минус сильнее, потому что темнить уже почти нечего.
    pub shadows: KnobRange,
    /// Света: `log2` наклона `m` у белого с обратным знаком (`+` светлее).
    pub highlights: KnobRange,
    /// Доля растянутого цвета: 0 — цвет не меняется, 1 — насыщенность тянется за светом (решение 243).
    pub color_follow: f32,
    /// Серый экрана, вокруг которого делятся тени и света: линейная яркость. Своя настройка слоя вида, а не
    /// `display_middle_gray` выбранной модели (слой не знает модели; как `wheels.zone_pivot_display_gray`).
    pub pivot_display_gray: f32,
}

impl Default for ToneShapeSettings {
    fn default() -> Self {
        Self {
            contrast: KnobRange::symmetric(2.32),
            midtones: KnobRange::symmetric(3.0),
            shadows: KnobRange { minus: 4.0, plus: 3.5 },
            highlights: KnobRange { minus: 3.5, plus: 4.0 },
            color_follow: 0.5,
            pivot_display_gray: 0.20,
        }
    }
}

impl ToneShapeSettings {
    /// Проверка чисел. Углы ручек проверять не нужно: кривая растёт при любых величинах (см. модуль).
    pub fn validate(&self) -> Result<(), InvalidLookModel> {
        InvalidLookModel::check_knob_range("tone.contrast", self.contrast, MAX_EDGE_LOG2)?;
        InvalidLookModel::check_knob_range("tone.midtones", self.midtones, MAX_EDGE_LOG2)?;
        InvalidLookModel::check_knob_range("tone.shadows", self.shadows, MAX_EDGE_LOG2)?;
        InvalidLookModel::check_knob_range("tone.highlights", self.highlights, MAX_EDGE_LOG2)?;
        InvalidLookModel::check_number("tone.color_follow", self.color_follow, 0.0, 1.0)?;
        let (darkest, brightest) = PIVOT_GRAY_LIMITS;
        InvalidLookModel::check_number("tone.pivot_display_gray", self.pivot_display_gray, darkest, brightest)
    }
}

/// Тон вида, разрешённый в числа кадра (общий вид для CPU и GPU: эти же числа кладутся в uniform).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LookTone {
    /// `q = √Y_g`: серый экрана в координате `t`.
    pub pivot: f32,
    /// `1 / q`.
    pub shadow_scale: f32,
    /// `1 / (1 − q)`.
    pub highlight_scale: f32,
    /// `m` теней: наклон у чёрного.
    pub shadow_slope: f32,
    /// `m` светов: наклон у белого (больше 1 — света темнее: расстояние до белого растянуто).
    pub highlight_slope: f32,
    /// `a`: наклон на сером.
    pub contrast_slope: f32,
    /// `s` средних тонов: наклон у чёрного.
    pub midtone_slope: f32,
    /// Доля растянутого цвета `0…1`.
    pub color_follow: f32,
}

impl LookTone {
    /// Разрешает четыре ручки тона. `None` — все в нуле: стадия пропускается целиком (нейтраль бит в бит).
    pub fn resolve(settings: &LookSettings, model: &ToneShapeSettings) -> Option<Self> {
        let knobs = [
            settings.contrast,
            settings.midtones,
            settings.shadows,
            settings.highlights,
        ];
        if knobs.iter().all(|knob| knob.is_neutral()) {
            return None;
        }
        let pivot = model.pivot_display_gray.sqrt();
        Some(Self {
            pivot,
            shadow_scale: 1.0 / pivot,
            highlight_scale: 1.0 / (1.0 - pivot),
            shadow_slope: model.shadows.amount(settings.shadows).exp2(),
            // Плюс делает света светлее: точки ближе к белому, значит расстояние от него у белого сжато.
            highlight_slope: (-model.highlights.amount(settings.highlights)).exp2(),
            contrast_slope: model.contrast.amount(settings.contrast).exp2(),
            midtone_slope: model.midtones.amount(settings.midtones).exp2(),
            color_follow: model.color_follow,
        })
    }

    /// Кривая одного канала линейного света экрана (формулы модуля; тот же порядок операций, что в GLSL).
    pub fn curve(&self, light: f32) -> f32 {
        if light <= 0.0 || light >= 1.0 {
            return light;
        }
        let t = light.sqrt();
        let shaped = if t <= self.pivot {
            let from_black = (t * self.shadow_scale).min(1.0);
            let zoned = zone_segment(from_black, self.shadow_slope);
            self.pivot * contrast_segment(zoned, self.contrast_slope)
        } else {
            let from_white = ((1.0 - t) * self.highlight_scale).min(1.0);
            let zoned = zone_segment(from_white, self.highlight_slope);
            1.0 - (1.0 - self.pivot) * contrast_segment(zoned, self.contrast_slope)
        };
        let moved = self.midtone_slope * shaped / (1.0 + (self.midtone_slope - 1.0) * shaped);
        moved * moved
    }

    /// Применяет стадию к свету `W` одного пикселя: смесь растянутого и подобного цвета (см. модуль).
    pub fn apply(&self, working_rgb: [f32; 3]) -> [f32; 3] {
        let brightest = working_rgb[0].max(working_rgb[1]).max(working_rgb[2]);
        let darkest = working_rgb[0].min(working_rgb[1]).min(working_rgb[2]);
        if brightest <= BLACK_FLOOR {
            return working_rgb;
        }
        let new_brightest = self.curve(brightest);
        let new_darkest = self.curve(darkest);
        let gain = new_brightest / brightest;
        let span = brightest - darkest;
        working_rgb.map(|channel| {
            // Отрицательный канал (за охватом) усиление не трогает: иначе в глубокой тени его унесло бы на тысячи.
            let similar = if channel > 0.0 { channel * gain } else { channel };
            let stretched = if span > 0.0 {
                new_darkest + (new_brightest - new_darkest) * ((channel - darkest) / span)
            } else {
                new_brightest
            };
            similar + self.color_follow * (stretched - similar)
        })
    }
}

/// `Z(u, m)`: отрезок зоны с наклоном `m` в нуле и 1 в единице.
fn zone_segment(unit: f32, slope_at_zero: f32) -> f32 {
    let bump = unit * (1.0 - unit);
    (unit * unit + slope_at_zero * bump) / (1.0 + (slope_at_zero - 1.0) * bump)
}

/// `K(u, a)`: отрезок контраста с наклоном `a` в единице и `1/a` в нуле.
fn contrast_segment(unit: f32, slope_at_one: f32) -> f32 {
    unit / (unit + slope_at_one * (1.0 - unit))
}
