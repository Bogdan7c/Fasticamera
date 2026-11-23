//! Цвета ободка колёс цвета (14b2): стиль из конфига экрана и расчёт стопов кольца для [`look_contract::LookFacts`].
//!
//! Экран не умеет OKLab, поэтому цвета ободка считает приложение: оттенок OKLCh при фиксированной светлоте → sRGB.
//!
//! Ободок — карта направления толчка, поэтому ОТТЕНОК стопа обязан быть точным. В отличие от точек полос
//! (там канал sRGB просто обрезается), здесь у оттенков на краю охвата sRGB снижается ЦВЕТНОСТЬ: ищется наибольшая
//! цветность не выше заданной, при которой цвет целиком лежит в охвате. Оттенок при этом не трогается.

use isp_model::color::chromaticity::Primaries;
use isp_model::color::oklab_working::OklabWorkspace;
use isp_model::transfer::srgb_encode;
use look_contract::{BandSwatch, MIN_WHEEL_RING_STOPS, WheelHueRing};
use serde::{Deserialize, Serialize};

use crate::look_facts::MAX_SWATCH_CHROMA;

const LOG_TARGET: &str = "fasticamera::look";

/// Шагов бинарного поиска цветности стопа. Каждый шаг вдвое сужает отрезок `[0, chroma]`; за 24 шага он короче
/// `0,4 / 2^24 ≈ 2·10⁻⁸` — на порядки меньше шага 8-битного канала (≈ 4·10⁻³ в OKLab), так что дальше искать
/// бессмысленно.
const CHROMA_SEARCH_STEPS: u32 = 24;

/// Верхняя граница числа стопов: больше пикселя на стоп растр кольца всё равно не различит.
pub const MAX_WHEEL_RING_STOPS: usize = 360;

/// Как красится ободок колеса цвета. Секция `camera_screen.wheel_ring`; числа — `[estimate]`, подбор глазами в 14j.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WheelRingStyle {
    /// Сколько цветовых стопов по кругу (оттенок стопа `i` — `i · 360 / N`). `[estimate]` 72: шаг 5° — ободок
    /// выглядит плавным, а растр кольца остаётся маленьким.
    pub stops: usize,
    /// Светлота OKLab L (0…1]. `[estimate]` 0,75: светлее точек полос, чтобы ободок был яркой подсказкой.
    pub lightness: f32,
    /// Наибольшая цветность OKLCh C [0…`MAX_SWATCH_CHROMA`]. `[estimate]` 0,13. У оттенков, не помещающихся в
    /// охват sRGB при этой светлоте, цветность стопа снижается, оттенок не трогается.
    pub chroma: f32,
}

impl Default for WheelRingStyle {
    fn default() -> Self {
        Self {
            stops: 72,
            lightness: 0.75,
            chroma: 0.13,
        }
    }
}

/// Почему секция `camera_screen.wheel_ring` неверна.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum WheelRingStyleError {
    #[error("camera_screen.wheel_ring.stops = {value}: нужно от {MIN_WHEEL_RING_STOPS} до {MAX_WHEEL_RING_STOPS}")]
    Stops { value: usize },
    #[error("camera_screen.wheel_ring.lightness = {value}: нужно больше 0 и не больше 1")]
    Lightness { value: f32 },
    #[error("camera_screen.wheel_ring.chroma = {value}: нужно от 0 до {MAX_SWATCH_CHROMA}")]
    Chroma { value: f32 },
    #[error(
        "camera_screen.wheel_ring.lightness = {lightness}: при такой светлоте у оттенка {hue_degrees}° в охват sRGB \
         влезает цветность лишь {fitting_chroma}, меньше {MIN_DISTINCT_STOP_CHROMA}: кольцо потеряет оттенки"
    )]
    LightnessLosesHues {
        lightness: f32,
        hue_degrees: f32,
        fitting_chroma: f32,
    },
}

/// Наименьшая цветность стопа, при которой оттенок ещё отличим от серого на ободке. `[estimate]` 0,03: в OKLab
/// порог заметного различия цвета порядка 0,02; чуть выше — чтобы стоп читался и на тёмном экране. Подбор глазами
/// в 14j. Если задана цветность стиля ниже порога, требуется только она сама (серое кольцо — выбор владельца).
pub const MIN_DISTINCT_STOP_CHROMA: f32 = 0.03;

/// Шаг оттенка при проверке светлоты, градусы: цветность по оттенку меняется плавно, 1° ловит и узкие углы охвата.
const GAMUT_CHECK_HUE_STEP_DEGREES: usize = 1;

impl WheelRingStyle {
    /// Проверка при разборе конфига: значения вне диапазона и NaN отвергаются целиком, а не обрезаются молча.
    pub fn validate(&self) -> Result<(), WheelRingStyleError> {
        if !(MIN_WHEEL_RING_STOPS..=MAX_WHEEL_RING_STOPS).contains(&self.stops) {
            return Err(WheelRingStyleError::Stops { value: self.stops });
        }
        if !(self.lightness > 0.0 && self.lightness <= 1.0) {
            return Err(WheelRingStyleError::Lightness { value: self.lightness });
        }
        if !(self.chroma >= 0.0 && self.chroma <= MAX_SWATCH_CHROMA) {
            return Err(WheelRingStyleError::Chroma { value: self.chroma });
        }
        self.validate_lightness_keeps_hues()
    }

    /// Проверка по факту, а не по произвольным границам: для каждого оттенка круга (шаг 1°) считается цветность,
    /// влезающая в sRGB при этой светлоте (тем же поиском, что у стопов). Если у какого-то оттенка она ниже
    /// порога различимости, стопы стали бы серыми, и кольцо перестало бы быть картой оттенков.
    /// Без матриц OKLab проверять нечем: ошибку матриц громко логирует расчёт кольца.
    fn validate_lightness_keeps_hues(&self) -> Result<(), WheelRingStyleError> {
        let Ok(workspace) = OklabWorkspace::new(&Primaries::SRGB) else {
            return Ok(());
        };
        let required = MIN_DISTINCT_STOP_CHROMA.min(self.chroma);
        for hue_step in (0..360).step_by(GAMUT_CHECK_HUE_STEP_DEGREES) {
            let hue_degrees = hue_step as f32;
            let (_, fitting_chroma) = fit_chroma(&workspace, self.lightness, hue_degrees, self.chroma);
            if fitting_chroma < required {
                return Err(WheelRingStyleError::LightnessLosesHues {
                    lightness: self.lightness,
                    hue_degrees,
                    fitting_chroma,
                });
            }
        }
        Ok(())
    }
}

/// Кольцо оттенков по стилю. Неверный стиль (в обход `validate`) не паникует: число стопов зажимается в допустимые
/// пределы — экран всегда получает корректное кольцо. Без матриц OKLab все стопы нейтральные, ошибка — в лог.
pub(crate) fn wheel_ring_for_style(workspace: Option<&OklabWorkspace>, style: &WheelRingStyle) -> WheelHueRing {
    const NEUTRAL_STOP: BandSwatch = BandSwatch { srgb: [128, 128, 128] };
    let stop_count = style.stops.clamp(MIN_WHEEL_RING_STOPS, MAX_WHEEL_RING_STOPS);
    let Some(workspace) = workspace else {
        tracing::error!(target: LOG_TARGET, "матрицы OKLab для sRGB не построены: ободок колёс серый");
        return WheelHueRing::solid(NEUTRAL_STOP);
    };
    let stops: Vec<BandSwatch> = (0..stop_count)
        .map(|index| stop_color(workspace, style, WheelHueRing::hue_degrees_of_stop(stop_count, index)))
        .collect();
    // Число стопов уже в пределах, поэтому отказ невозможен; запасной путь — одноцветное кольцо, не паника.
    WheelHueRing::new(stops).unwrap_or_else(|_| WheelHueRing::solid(NEUTRAL_STOP))
}

/// Цвет стопа: наибольшая цветность `≤ style.chroma`, при которой линейный RGB целиком в [0, 1].
fn stop_color(workspace: &OklabWorkspace, style: &WheelRingStyle, hue_degrees: f32) -> BandSwatch {
    let (best, _) = fit_chroma(workspace, style.lightness, hue_degrees, style.chroma);
    // `srgb_encode` обрезает вход в 0…1 (здесь лишь гасит ошибку округления на границе); 0…1 → 0…255 с округлением.
    BandSwatch {
        srgb: best.map(|channel| (srgb_encode(channel) * 255.0).round() as u8),
    }
}

/// Наибольшая цветность `≤ max_chroma`, при которой точка (светлота, оттенок) целиком в охвате sRGB: бинарный
/// поиск (охват выпуклый по цветности вдоль луча от серой оси). Возвращает линейный RGB и найденную цветность.
/// Если помещается заданная цветность — берётся она сама.
fn fit_chroma(workspace: &OklabWorkspace, lightness: f32, hue_degrees: f32, max_chroma: f32) -> ([f32; 3], f32) {
    let (sin, cos) = hue_degrees.to_radians().sin_cos();
    let linear_at = |chroma: f32| workspace.to_rgb([lightness, chroma * cos, chroma * sin]);
    let in_gamut = |linear: [f32; 3]| linear.iter().all(|channel| (0.0..=1.0).contains(channel));

    let full = linear_at(max_chroma);
    if in_gamut(full) {
        return (full, max_chroma);
    }
    // Серая точка той же светлоты — запасной ответ, если не поместилось ничего (светлота ≈ 1 с ошибкой округления).
    let (mut best, mut best_chroma) = (linear_at(0.0), 0.0_f32);
    let mut too_much = max_chroma;
    for _ in 0..CHROMA_SEARCH_STEPS {
        let middle = 0.5 * (best_chroma + too_much);
        let candidate = linear_at(middle);
        if in_gamut(candidate) {
            best_chroma = middle;
            best = candidate;
        } else {
            too_much = middle;
        }
    }
    (best, best_chroma)
}

#[cfg(test)]
#[path = "look_wheel_ring_tests.rs"]
mod tests;
