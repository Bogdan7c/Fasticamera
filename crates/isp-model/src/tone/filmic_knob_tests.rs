//! Основа filmic: ручки тона (направления, нейтраль, края × подтяжка AE) и экспорт чисел для шейдера.

use look_contract::{LookKnob, LookSettings};

use super::tests::{ALL_NORMS, base_for_norm, base_with};
use super::*;
use crate::color::matrix::PixelMatrix;
use crate::exposure::ExposureGain;
use crate::look::model::LookModelSettings;
use crate::look::tone_amounts;
use crate::tone::knobs::{ToneKnob, ToneKnobAmounts, ToneModelKind, ToneModelSettings, apply_knobs_strict};
use crate::tone::working_io::WorkingInputGamut;

// ---- Ручки тона ----

fn y_at(settings: &FilmicSettings, e: f64) -> f32 {
    let base = base_with(*settings);
    base.curve_value((f64::from(settings.scene_middle_gray) * e.exp2()) as f32, 1.0)
}

#[test]
fn neutral_knobs_keep_the_settings_and_the_exported_numbers_bit_for_bit() {
    let settings = FilmicSettings::default();
    let strict = apply_knobs_strict(&settings, &ToneKnobAmounts::default()).unwrap();
    assert_eq!(strict, settings);
    assert_eq!(base_with(strict).parameters(), base_with(settings).parameters());
    assert_eq!(FilmicSettings::KIND, ToneModelKind::Filmic);
}

#[test]
fn contrast_knob_changes_the_slope_and_keeps_grey() {
    let settings = FilmicSettings::default();
    let steeper = settings.with_knob(ToneKnob::Contrast, 0.3).unwrap();
    assert_eq!(y_at(&steeper, 0.0), y_at(&settings, 0.0), "серый не должен двигаться");
    assert!(
        y_at(&steeper, 0.8) > y_at(&settings, 0.8) + 0.01,
        "света должны стать контрастнее"
    );
    assert!(
        y_at(&steeper, -0.8) < y_at(&settings, -0.8) - 0.01,
        "тени должны стать контрастнее"
    );
}

#[test]
fn midtones_knob_moves_the_display_grey_only() {
    let settings = FilmicSettings::default();
    let brighter = settings.with_knob(ToneKnob::Midtones, 0.3).unwrap();
    let grey = brighter.scene_middle_gray;
    let display = base_with(brighter).map_scene_pixel([grey; 3], 1.0);
    let expected = settings.display_middle_gray * 0.3_f32.exp2();
    assert!((display[0] - expected).abs() < 1.5e-6, "{display:?} против {expected}");
    assert!(display[0] > settings.display_middle_gray);
}

#[test]
fn shadows_and_highlights_knobs_change_only_their_own_part_of_the_curve() {
    let settings = FilmicSettings::default();
    let base = base_with(settings);
    let (below, above) = base.linear_extent_ev();
    let lighter_shadows = settings.with_knob(ToneKnob::Shadows, 0.6).unwrap();
    let lighter_highlights = settings.with_knob(ToneKnob::Highlights, 0.6).unwrap();
    // «+» = светлее на своей стороне.
    assert!(y_at(&lighter_shadows, -4.0) > y_at(&settings, -4.0) + 1e-3);
    assert!(y_at(&lighter_highlights, 2.7) > y_at(&settings, 2.7) + 1e-3);
    let darker_shadows = settings.with_knob(ToneKnob::Shadows, -0.6).unwrap();
    assert!(y_at(&darker_shadows, -4.0) < y_at(&settings, -4.0) - 1e-3);
    // Другая сторона и линейный участок — те же биты.
    for e in [-1.0_f64, 0.0, 1.0, 2.7, 3.0] {
        assert!(e > -f64::from(below));
        assert_eq!(y_at(&lighter_shadows, e), y_at(&settings, e), "тени тронули e={e}");
    }
    for e in [-6.0_f64, -4.0, -1.0, 0.0, 0.5] {
        assert!(e < f64::from(above));
        assert_eq!(y_at(&lighter_highlights, e), y_at(&settings, e), "света тронули e={e}");
    }
}

#[test]
fn tone_knob_edges_times_ae_gain_edges_stay_solvable() {
    // Все углы {−1, 0, +1}⁴ ручек (диапазоны модели вида по умолчанию) × подтяжка AE {−0,5; 0; +0,5} EV.
    let model = LookModelSettings::default();
    let base_settings = FilmicSettings::default();
    let mut smallest_margin = f64::MAX;
    for gain_ev in [-0.5_f32, 0.0, 0.5] {
        let gain = ExposureGain::from_ev(gain_ev).unwrap();
        for corner in 0..81_u32 {
            let value = |index: u32| LookKnob::new(((corner / 3_u32.pow(index)) % 3) as f32 - 1.0).unwrap();
            let look = LookSettings {
                contrast: value(0),
                midtones: value(1),
                shadows: value(2),
                highlights: value(3),
                ..LookSettings::NEUTRAL
            };
            let settings = apply_knobs_strict(&base_settings, &tone_amounts(&model, &look))
                .unwrap()
                .with_exposure_gain(gain);
            settings
                .validate()
                .unwrap_or_else(|error| panic!("угол {corner}, AE {gain_ev:+}: {error}"));
            // Запас по более строгому условию плеча: s·Δ_w / (1 − y_p) − 1.
            let y_p = f64::from(settings.display_middle_gray).powf(1.0 / f64::from(settings.display_gamma));
            let headroom = (1.0 / f64::from(settings.scene_middle_gray)).log2();
            smallest_margin = smallest_margin.min(f64::from(settings.slope) * headroom / (1.0 - y_p) - 1.0);
        }
    }
    // Запас на худшем углу (контраст −, средние −, AE −0,5 EV) — 2,4 %: кривая строится, но близко к границе;
    // уменьшение запаса ниже 1 % — повод пересмотреть умолчания.
    assert!(smallest_margin > 0.01, "запас плеча на худшем углу {smallest_margin}");
}

// ---- Экспорт чисел для шейдера ----

/// Пиксель, собранный ТОЛЬКО из [`FilmicParameters`] (как будет в шейдере), в том же порядке операций.
fn rebuild_from_parameters(parameters: &FilmicParameters, scene: [f32; 3], white: f32) -> [f32; 3] {
    assert!(
        parameters.input_compression.is_none(),
        "тест — для входа «обрезка нулём»"
    );
    if scene.iter().any(|channel| channel.is_nan()) {
        return [0.0; 3];
    }
    let light = scene.map(|channel| channel.max(0.0).clamp(0.0, MAX_SCENE_LIGHT));
    let curve = |x: f32| -> f32 {
        if x.is_nan() || x <= 0.0 {
            return 0.0;
        }
        let e = (x / parameters.grey).log2();
        let delta = (white.max(MIN_WHITE_LEVEL) / parameters.grey).log2();
        if e >= delta {
            return 1.0;
        }
        let y = if e > parameters.linear_above {
            let rise = e - parameters.linear_above;
            let width = delta - parameters.linear_above;
            let ratio = parameters.slope * width / parameters.shoulder_headroom;
            let excess = ratio.powf(parameters.shoulder_power) - 1.0;
            let blend = (rise / width).powf(parameters.shoulder_power) * excess;
            parameters.shoulder_value + parameters.slope * rise / (1.0 + blend).powf(parameters.shoulder_inverse_power)
        } else if e >= -parameters.linear_below {
            parameters.gray_output + parameters.slope * e
        } else {
            let depth = -parameters.linear_below - e;
            if depth >= parameters.toe_width {
                return 0.0;
            }
            let blend = (depth / parameters.toe_width).powf(parameters.toe_power) * parameters.toe_excess;
            parameters.toe_value - parameters.slope * depth / (1.0 + blend).powf(parameters.toe_inverse_power)
        };
        y.clamp(0.0, 1.0)
    };
    if parameters.norm == FilmicNorm::PerChannel {
        return light.map(|channel| curve(channel).powf(parameters.gamma));
    }
    let largest = light[0].max(light[1]).max(light[2]);
    let row = parameters.luminance_row;
    let norm = match parameters.norm {
        FilmicNorm::PerChannel | FilmicNorm::MaxRgb => largest,
        FilmicNorm::Luminance => row[0] * light[0] + row[1] * light[1] + row[2] * light[2],
        FilmicNorm::PowerNorm => {
            if largest <= 0.0 {
                0.0
            } else {
                let unit = light.map(|channel| channel / largest);
                let squares: f32 = unit.iter().map(|u| u * u).sum();
                let cubes: f32 = unit.iter().map(|u| u * u * u).sum();
                largest * cubes / squares
            }
        }
    };
    if norm <= 0.0 {
        return [0.0; 3];
    }
    let curved = curve(norm);
    let display = curved.powf(parameters.gamma);
    let t = ((curved - parameters.desaturation_onset) / (1.0 - parameters.desaturation_onset)).clamp(0.0, 1.0);
    let weight = t * t * t * (t * (6.0 * t - 15.0) + 10.0);
    let coloured = light.map(|channel| display * (channel / norm));
    let mixed = if weight <= 0.0 {
        coloured
    } else if weight >= 1.0 {
        [display; 3]
    } else {
        let [lightness, a, b] = parameters.oklab.to_lab(coloured);
        let [white_lightness, _, _] = parameters.oklab.to_lab([display; 3]);
        let keep = 1.0 - weight;
        parameters
            .oklab
            .to_rgb([keep * lightness + weight * white_lightness, keep * a, keep * b])
    };
    let output = PixelMatrix {
        rows: parameters.working_to_output,
    }
    .apply(mixed);
    let largest_output = output.iter().copied().fold(1.0_f32, f32::max);
    if largest_output <= 1.0 + FIT_TOLERANCE {
        mixed
    } else {
        mixed.map(|channel| channel / largest_output)
    }
}

#[test]
fn exported_parameters_reproduce_the_pixel_formula_bit_for_bit_for_every_norm() {
    let pixels = [
        [0.075_f32; 3],
        [0.2, 0.05, 0.01],
        [0.6, 0.3, 0.1],
        [0.9, 0.05, 0.8],
        [0.02, 0.4, 0.9],
        [1.0, 0.5, 0.2],
        [3.0, 0.2, 0.0],
        [0.0005, 0.0002, 0.0001],
        [1.4, 1.4, 1.4],
        [-0.1, 0.3, 0.2],
    ];
    for norm in ALL_NORMS {
        let base = base_for_norm(norm);
        let parameters = base.parameters();
        assert_eq!(parameters.norm, norm);
        for white in [1.0_f32, 1.4, 2.2] {
            for pixel in pixels {
                assert_eq!(
                    base.map_scene_pixel(pixel, white),
                    rebuild_from_parameters(&parameters, pixel, white),
                    "{norm:?} L={white} {pixel:?}"
                );
            }
        }
    }
}

#[test]
fn desaturation_keeps_the_hue_of_the_coloured_pixel_in_oklab() {
    // Шаг обесцвечивания — смесь в OKLab к белому: направление (a, b) не меняется, цветность убывает, светлота
    // растёт к белой. Смесь в линейном свете сдвигала бы оттенок (замер: 5–9° на оранжевом).
    let base = base_for_norm(FilmicNorm::MaxRgb);
    let oklab = base.parameters().oklab;
    let grey = FilmicSettings::default().scene_middle_gray;
    let mut previous_chroma = f32::MAX;
    let mut reference_direction: Option<(f32, f32)> = None;
    for step in 0..40 {
        // Оранжевый (в W): возрастающая яркость от +2 до +3,5 EV, где вес обесцвечивания между 0 и 1.
        let scale = grey * (2.0 + 0.0375 * step as f32).exp2();
        let pixel = [scale, scale * 0.45, scale * 0.03];
        let display = base.map_scene_pixel(pixel, 1.0);
        let [_, a, b] = oklab.to_lab(display);
        let chroma = a.hypot(b);
        if chroma > 0.01 {
            let direction = (a / chroma, b / chroma);
            let reference = *reference_direction.get_or_insert(direction);
            assert!(
                (direction.0 - reference.0).hypot(direction.1 - reference.1) < 2e-3,
                "шаг {step}: оттенок ушёл {direction:?} против {reference:?}"
            );
        }
        assert!(
            chroma <= previous_chroma + 1e-4,
            "шаг {step}: цветность растёт {previous_chroma} -> {chroma}"
        );
        previous_chroma = chroma;
    }
    assert!(
        previous_chroma < 0.02,
        "к +3,5 EV оранжевый должен почти обесцветиться: {previous_chroma}"
    );
}

#[test]
fn input_gamut_compression_option_reaches_the_shared_input_step() {
    let compress = FilmicSettings {
        input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
        ..FilmicSettings::default()
    };
    let base = base_with(compress);
    assert!(base.parameters().input_compression.is_some());
    // Цвет за охватом (отрицательный канал): сжатие даёт другой результат, чем обрезка нулём.
    let beyond = [0.3_f32, 0.1, -0.02];
    let clipped = base_with(FilmicSettings::default()).map_scene_pixel(beyond, 1.0);
    assert_ne!(base.map_scene_pixel(beyond, 1.0), clipped);
}
