//! «Подслащивание» RRT: свечение (glow) и красный модификатор (red modifier) ACES 1.x.
//!
//! Источник и лицензия — в доке модуля [`super`] (`tone::aces`): формулы и константы взяты из Academy aces-dev
//! v1.3 (`RRT.ctl`, `ACESlib.RRT_Common.ctl`, `ACESlib.Utilities_Color.ctl`).
//!
//! # Шаги (вход — линейный ACES2065-1 / AP0, где средний серый = 0,18)
//! ```text
//! sat   = (max(max(rgb), ε) − max(min(rgb), ε)) / max(max(rgb), 10⁻²)      — «насыщенность» ACES (ε = 10⁻¹⁰)
//! yc    = (r + g + b + 1,75·√(b(b−g) + g(g−r) + r(r−b))) / 3                — яркость-прокси с вкладом цветности
//! s     = sigmoid((sat − 0,4)/0,2)                                          — 0 у серого, 1 у насыщенного
//! glow  = 1 + glow_fwd(yc, G·s, 0,08)                                       — G = glow_gain (в ACES 0,05)
//! rgb   = glow · rgb
//! hue   = atan2(√3·(g − b), 2r − g − b)  (градусы, −180…180; у серого — вес 0)
//! w     = cubic_basis_shaper(hue, 135°)                                     — вес вокруг красного, 0…1
//! r     = r + w · sat · (0,03 − r) · (1 − R)                                — R = red_modifier_scale (в ACES 0,82)
//! ```
//! `glow_fwd(yc, g, m)`: `g` при `yc ≤ 2m/3`, `0` при `yc ≥ 2m`, иначе `g·(m/yc − ½)`. `sigmoid(x)` — гладкий
//! переход от 0 к 1 на `x ∈ [−2, 2]`: `(1 + sign(x)·(1 − t²))/2`, `t = max(1 − |x|/2, 0)`.
//!
//! # Отличие от CTL, ради паритета с GPU
//! Оттенок в CTL считается `atan2` из библиотеки. Шейдерные `atan`/`atan2` допускают ошибку до тысяч ULP, а вес
//! красного входит в тон. Поэтому здесь свой многочлен арктангенса ([`atan2_polynomial`], формула 4.4.49
//! Абрамовица и Стиген, ошибка ≤ 2·10⁻⁸ рад) — одинаковый на CPU и в шейдере. Центрирование оттенка
//! (`center_hue` в CTL) для `RRT_RED_HUE = 0` — это просто `atan2` в диапазоне −180…180°, отдельный шаг не нужен.

use serde::Serialize;

/// Середина свечения `RRT_GLOW_MID`.
const GLOW_MIDPOINT: f32 = 0.08;
/// Опора красного модификатора `RRT_RED_PIVOT`.
const RED_PIVOT: f32 = 0.03;
/// Полная ширина веса красного, градусы (`RRT_RED_WIDTH`); центр веса — 0° (`RRT_RED_HUE`).
const RED_WIDTH_DEGREES: f32 = 135.0;
/// Пол для насыщенности (`TINY` в CTL) и знаменателя.
const SATURATION_FLOOR: f32 = 1e-10;
const SATURATION_DENOMINATOR_FLOOR: f32 = 1e-2;
/// Вес радиуса в `rgb_2_yc` (по умолчанию в CTL).
const YC_RADIUS_WEIGHT: f32 = 1.75;

/// Настраиваемые числа шага (остальное — константы RRT).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SweetenerParameters {
    /// Сила свечения `RRT_GLOW_GAIN` (в ACES 0,05; 0 — выключено).
    pub glow_gain: f32,
    /// Масштаб красного модификатора `RRT_RED_SCALE` (в ACES 0,82; 1 — выключено).
    pub red_modifier_scale: f32,
}

/// Насыщенность по ACES (`rgb_2_saturation`).
fn saturation(rgb: [f32; 3]) -> f32 {
    let largest = rgb[0].max(rgb[1]).max(rgb[2]);
    let smallest = rgb[0].min(rgb[1]).min(rgb[2]);
    (largest.max(SATURATION_FLOOR) - smallest.max(SATURATION_FLOOR)) / largest.max(SATURATION_DENOMINATOR_FLOOR)
}

/// Яркость-прокси `YC` (`rgb_2_yc`). Подкоренное `≥ 0` математически; округление может дать `−0` → защита.
fn luma_chroma_proxy(rgb: [f32; 3]) -> f32 {
    let [r, g, b] = rgb;
    let chroma = (b * (b - g) + g * (g - r) + r * (r - b)).max(0.0).sqrt();
    (b + g + r + YC_RADIUS_WEIGHT * chroma) / 3.0
}

/// Гладкий переход 0…1 на `x ∈ [−2, 2]` (`sigmoid_shaper`).
fn sigmoid_shaper(x: f32) -> f32 {
    let t = (1.0 - (x / 2.0).abs()).max(0.0);
    let sign = if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    };
    (1.0 + sign * (1.0 - t * t)) / 2.0
}

/// Добавка свечения (`glow_fwd`) при входном усилении `gain`.
fn glow_addition(luma_chroma: f32, gain: f32) -> f32 {
    if luma_chroma <= 2.0 / 3.0 * GLOW_MIDPOINT {
        gain
    } else if luma_chroma >= 2.0 * GLOW_MIDPOINT {
        0.0
    } else {
        gain * (GLOW_MIDPOINT / luma_chroma - 0.5)
    }
}

/// Арктангенс на `[0, 1]`: Абрамовиц и Стиген, формула 4.4.49 (ошибка ≤ 2·10⁻⁸ рад).
fn atan_unit_interval(x: f32) -> f32 {
    let x2 = x * x;
    // Схема Горнера по степеням x² (от старшей к младшей).
    let series = 0.002_866_225_7_f32;
    let series = series * x2 - 0.016_165_737;
    let series = series * x2 + 0.042_909_614;
    let series = series * x2 - 0.075_289_64;
    let series = series * x2 + 0.106_562_64;
    let series = series * x2 - 0.142_089;
    let series = series * x2 + 0.199_935_51;
    let series = series * x2 - 0.333_331_45;
    (series * x2 + 1.0) * x
}

/// `atan2(y, x)` на многочлене, радианы `(−π, π]`; `(0, 0)` даёт 0.
pub fn atan2_polynomial(y: f32, x: f32) -> f32 {
    let (abs_x, abs_y) = (x.abs(), y.abs());
    let largest = abs_x.max(abs_y);
    if largest <= 0.0 {
        return 0.0;
    }
    let ratio = abs_x.min(abs_y) / largest;
    let mut angle = atan_unit_interval(ratio);
    if abs_y > abs_x {
        angle = std::f32::consts::FRAC_PI_2 - angle;
    }
    if x < 0.0 {
        angle = std::f32::consts::PI - angle;
    }
    if y < 0.0 {
        angle = -angle;
    }
    angle
}

/// Весовая функция кубического B-сплайна шириной `width` (градусы) вокруг нуля (`cubic_basis_shaper`).
fn cubic_basis_shaper(hue_degrees: f32, width: f32) -> f32 {
    let half = width / 2.0;
    if !(hue_degrees > -half && hue_degrees < half) {
        return 0.0;
    }
    let knot_coordinate = (hue_degrees + half) * 4.0 / width;
    let segment = knot_coordinate as u32;
    let t = knot_coordinate - segment as f32;
    // Базисные функции равномерного кубического B-сплайна на четырёх участках (столбцы матрицы `M` из CTL).
    let value = match segment {
        0 => t * t * t / 6.0,
        1 => (-3.0 * t * t * t + 3.0 * t * t + 3.0 * t + 1.0) / 6.0,
        2 => (3.0 * t * t * t - 6.0 * t * t + 4.0) / 6.0,
        _ => (-t * t * t + 3.0 * t * t - 3.0 * t + 1.0) / 6.0,
    };
    value * 1.5
}

/// Вес красного модификатора по оттенку цвета; у серого (все каналы равны) оттенка нет — вес 0.
fn red_hue_weight(rgb: [f32; 3]) -> f32 {
    let [r, g, b] = rgb;
    if r == g && g == b {
        return 0.0;
    }
    let hue_degrees = atan2_polynomial(3.0_f32.sqrt() * (g - b), 2.0 * r - g - b) * (180.0 / std::f32::consts::PI);
    cubic_basis_shaper(hue_degrees, RED_WIDTH_DEGREES)
}

/// Свечение и красный модификатор для пикселя в ACES2065-1 (AP0). Результат ещё не зажат (зажим — у вызывающего).
pub fn apply_sweeteners(parameters: &SweetenerParameters, aces: [f32; 3]) -> [f32; 3] {
    // Насыщенность и яркость-прокси считаются по входу; свечение масштабирует цвет целиком.
    let saturation = saturation(aces);
    let luma_chroma = luma_chroma_proxy(aces);
    let sigmoid = sigmoid_shaper((saturation - 0.4) / 0.2);
    let glow = 1.0 + glow_addition(luma_chroma, parameters.glow_gain * sigmoid);
    let glowing = aces.map(|channel| glow * channel);
    // Красный модификатор: оттенок уже после свечения, насыщенность — до него (как в RRT.ctl).
    let weight = red_hue_weight(glowing);
    let red = glowing[0] + weight * saturation * (RED_PIVOT - glowing[0]) * (1.0 - parameters.red_modifier_scale);
    [red, glowing[1], glowing[2]]
}
