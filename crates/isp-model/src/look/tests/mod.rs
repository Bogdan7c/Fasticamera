//! Тесты слоя вида. Общие помощники; сами проверки — по темам в соседних файлах.

mod bands_tests;
mod color_tests;
mod end_to_end_tests;
mod model_tests;
mod tone_tests;
mod wheels_tests;
mod white_tests;

use look_contract::{BandAdjustment, BandAdjustments, HueBandId, LookKnob, LookSettings};

use crate::color::chromaticity::Primaries;
use crate::color::oklab_working::OklabWorkspace;

/// Ручка со значением (в тестах значения заведомо допустимы).
pub(super) fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

pub(super) fn band_id(raw: &str) -> HueBandId {
    HueBandId::new(raw).unwrap()
}

/// Правка одной полосы: `(оттенок, насыщенность, яркость)`.
pub(super) fn band_adjustment(id: &str, hue: f32, saturation: f32, luminance: f32) -> BandAdjustments {
    BandAdjustments::new(vec![BandAdjustment {
        band: band_id(id),
        hue: knob(hue),
        saturation: knob(saturation),
        luminance: knob(luminance),
    }])
    .unwrap()
}

pub(super) fn settings_with(edit: impl FnOnce(&mut LookSettings)) -> LookSettings {
    let mut settings = LookSettings::NEUTRAL;
    edit(&mut settings);
    settings
}

/// OKLab Rec. 2020 для замеров.
pub(super) fn workspace() -> OklabWorkspace {
    OklabWorkspace::new(&Primaries::REC_2020).unwrap()
}

/// Светлота, цветность, оттенок в градусах `[0, 360)` цвета `W`.
pub(super) fn lch(rgb: [f32; 3]) -> (f32, f32, f32) {
    let [lightness, a, b] = workspace().to_lab(rgb);
    (lightness, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0))
}

/// Цвет `W` по светлоте, цветности и оттенку в градусах.
pub(super) fn from_lch(lightness: f32, chroma: f32, hue_degrees: f32) -> [f32; 3] {
    let (sine, cosine) = hue_degrees.to_radians().sin_cos();
    workspace().to_rgb([lightness, chroma * cosine, chroma * sine])
}

pub(super) fn hue_distance(first: f32, second: f32) -> f32 {
    let difference = (first - second).rem_euclid(360.0);
    difference.min(360.0 - difference)
}
