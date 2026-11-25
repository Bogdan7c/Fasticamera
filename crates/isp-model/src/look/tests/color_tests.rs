use super::*;
use crate::color::chromaticity::Primaries;
use crate::look::color::LookColorParams;
use crate::look::model::LookModelSettings;
use crate::look::stage::apply_look;
use crate::look::{LookControls, LookParams};

fn resolve_color(settings: &LookSettings) -> Option<LookColorParams> {
    LookColorParams::resolve(settings, &LookModelSettings::default(), &Primaries::REC_2020)
        .unwrap()
        .params
}

/// Цветной объект средней светлоты.
const ORANGE_HUE: f32 = 60.0;

fn colour(hue: f32, chroma: f32) -> [f32; 3] {
    from_lch(0.6, chroma, hue)
}

#[test]
fn saturation_multiplies_chroma_by_the_formula_and_keeps_hue_at_any_strength() {
    let model = LookModelSettings::default();
    for hue in [ORANGE_HUE, 142.0, 264.0, 345.0] {
        let source = colour(hue, 0.12);
        let (lightness, chroma, source_hue) = lch(source);
        for k in [-0.7_f32, -0.2, 0.3, 1.0] {
            let params = resolve_color(&settings_with(|s| s.saturation = knob(k))).unwrap();
            let (new_lightness, new_chroma, new_hue) = lch(params.apply(source));
            let expected = chroma * (1.0 + model.saturation.amount(knob(k)));
            assert!(
                (new_chroma - expected).abs() < 2e-4,
                "h={hue} k={k}: {new_chroma} против {expected}"
            );
            assert!((new_lightness - lightness).abs() < 1e-4, "светлота меняется");
            // Порог оттенка 0,3°: пропорциональная цветность оттенок не вращает.
            assert!(
                hue_distance(new_hue, source_hue) < 0.3,
                "h={hue} k={k}: оттенок {source_hue} -> {new_hue}"
            );
        }
    }
}

#[test]
fn grey_stays_grey_under_saturation_vibrance_and_bands() {
    let heavy = settings_with(|s| {
        s.saturation = knob(1.0);
        s.vibrance = knob(1.0);
        s.bands = band_adjustment("red", 1.0, 1.0, 1.0);
    });
    let params = resolve_color(&heavy).unwrap();
    assert!(params.bands.is_some());
    for step in 0..60 {
        let level = 0.002 * 1.15_f32.powi(step);
        let out = params.apply([level; 3]);
        let spread = out.iter().copied().fold(f32::MIN, f32::max) - out.iter().copied().fold(f32::MAX, f32::min);
        assert!(spread < 2e-5 * level.max(1.0), "серый {level} окрасился: {out:?}");
        // Яркость серого не уехала от полос (яркость полосы не действует на серый).
        assert!(
            (out[0] - level).abs() < 3e-5 * level.max(1.0) + 1e-6,
            "{level} -> {out:?}"
        );
    }
}

#[test]
fn full_negative_saturation_removes_colour() {
    let params = resolve_color(&settings_with(|s| s.saturation = knob(-1.0))).unwrap();
    let (_, chroma, _) = lch(params.apply(colour(30.0, 0.2)));
    assert!(chroma < 5e-4, "{chroma}");
}

#[test]
fn vibrance_changes_skin_less_than_pure_blue_and_lifts_dull_colours_more_than_vivid() {
    let params = resolve_color(&settings_with(|s| s.vibrance = knob(0.8))).unwrap();
    let skin_hue = LookModelSettings::default().skin.hue_center_degrees;
    let gain = |hue: f32, chroma: f32| {
        let (_, before, _) = lch(colour(hue, chroma));
        let (_, after, _) = lch(params.apply(colour(hue, chroma)));
        after / before
    };
    // Одинаковая цветность и ручка: у кожи подъём слабее, чем у синего.
    let (skin, blue) = (gain(skin_hue, 0.06), gain(264.0, 0.06));
    assert!(blue > 1.15, "синий почти не вырос: {blue}");
    assert!(skin < 1.0 + (blue - 1.0) * 0.5, "кожа {skin} против синего {blue}");
    assert!(skin > 1.0, "кожа не должна падать при плюсовой сочности");
    // Уже насыщенное подавляется: тусклое растёт сильнее яркого того же оттенка.
    assert!(gain(264.0, 0.04) > gain(264.0, 0.2) + 0.1);
    // Минусовая сочность убавляет цвет, оттенок не меняется.
    let minus = resolve_color(&settings_with(|s| s.vibrance = knob(-0.8))).unwrap();
    let (_, cut, cut_hue) = lch(minus.apply(colour(150.0, 0.06)));
    assert!(cut < lch(colour(150.0, 0.06)).1 && hue_distance(cut_hue, 150.0) < 0.3);
}

#[test]
fn a_band_moves_its_own_colours_and_barely_touches_the_far_ones() {
    let model = LookModelSettings::default();
    let params = resolve_color(&settings_with(|s| s.bands = band_adjustment("green", 1.0, 0.0, 0.0))).unwrap();
    // Центр зелёной полосы — 142°: сдвиг оттенка на край ручки.
    let (_, _, hue) = lch(params.apply(colour(142.0, 0.15)));
    assert!(
        (hue_distance(hue, 142.0) - model.band_hue_degrees.plus).abs() < 0.6,
        "зелёный сдвинулся на {}°",
        hue_distance(hue, 142.0)
    );
    // Дальние цвета (красный 29°, синий 264°, пурпурный 345°) не трогаются.
    for far in [29.0_f32, 264.0, 345.0] {
        let (_, _, moved) = lch(params.apply(colour(far, 0.15)));
        assert!(
            hue_distance(moved, far) < 0.2,
            "{far}°: сдвиг {}°",
            hue_distance(moved, far)
        );
    }
    // Насыщенность и яркость полосы: свои оси.
    let desaturate = resolve_color(&settings_with(|s| s.bands = band_adjustment("blue", 0.0, -1.0, 0.0))).unwrap();
    let (_, chroma, _) = lch(desaturate.apply(colour(264.0, 0.15)));
    assert!(
        (chroma - 0.15 * (1.0 - model.band_saturation.minus)).abs() < 2e-3,
        "{chroma}"
    );
    let brighten = resolve_color(&settings_with(|s| s.bands = band_adjustment("blue", 0.0, 0.0, 1.0))).unwrap();
    let (lightness, _, _) = lch(brighten.apply(colour(264.0, 0.15)));
    assert!(
        (lightness - 0.6 * (1.0 + model.band_luminance.plus)).abs() < 3e-3,
        "{lightness}"
    );
    let (far_lightness, _, _) = lch(brighten.apply(colour(60.0, 0.15)));
    assert!(
        (far_lightness - 0.6).abs() < 3e-3,
        "дальний цвет изменил светлоту: {far_lightness}"
    );
}

#[test]
fn hue_sweep_through_window_boundaries_and_the_wrap_has_no_steps() {
    let model = LookModelSettings::default();
    let adjustments = look_contract::BandAdjustments::new(
        model
            .bands
            .iter()
            .enumerate()
            .map(|(index, band)| look_contract::BandAdjustment {
                band: band.id.clone(),
                hue: knob(if index % 2 == 0 { 1.0 } else { -1.0 }),
                saturation: knob(if index % 2 == 0 { -1.0 } else { 1.0 }),
                luminance: knob(0.0),
            })
            .collect(),
    )
    .unwrap();
    let params = resolve_color(&settings_with(|s| s.bands = adjustments)).unwrap();
    let steps = 7200;
    let mut previous = lch(params.apply(colour(0.0, 0.15)));
    let mut worst_hue_step: f32 = 0.0;
    let mut worst_chroma_step: f32 = 0.0;
    for step in 1..=steps {
        let hue = (step % steps) as f32 / steps as f32 * 360.0;
        let source_step = 360.0 / steps as f32;
        let current = lch(params.apply(colour(hue, 0.15)));
        // Выходной оттенок по шагу входного: производная ограничена (оттенок + сдвиг ≤ 30°, окна ≥ 31°).
        worst_hue_step = worst_hue_step.max(hue_distance(current.2, previous.2) / source_step);
        worst_chroma_step = worst_chroma_step.max((current.1 - previous.1).abs());
        previous = current;
    }
    // d(выходной оттенок)/d(входной) = 1 + d(сдвиг)/dθ ≤ 1 + (π/2)·(60°/31°)·... — потолок 6, ступенька дала бы сотни.
    assert!(worst_hue_step < 6.0, "производная оттенка {worst_hue_step}");
    assert!(worst_chroma_step < 0.002, "скачок цветности {worst_chroma_step}");
}

#[test]
fn neutral_colour_knobs_skip_the_stage_and_leave_the_image_bit_for_bit() {
    assert!(resolve_color(&LookSettings::NEUTRAL).is_none());
    // Правка только неизвестной модели полосы — тоже нейтраль после разрешения.
    assert!(
        resolve_color(&settings_with(
            |s| s.bands = band_adjustment("nonexistent", 1.0, 1.0, 1.0)
        ))
        .is_none()
    );
    let look = LookParams::resolve(&LookControls::default(), &Primaries::REC_2020).unwrap();
    assert!(look.is_neutral());
    let image = crate::image::RgbImage::from_pixels(
        camera_contract::geometry::PixelSize::new(2, 1),
        vec![[0.3, 0.2, 0.9], [1.4, -0.1, 0.0]],
        crate::image::space::WorkingDisplay,
    )
    .unwrap();
    assert_eq!(apply_look(&image, &look), image);
}

#[test]
fn colour_stage_actually_changes_a_coloured_image_when_touched() {
    let controls = LookControls::new(
        settings_with(|s| s.saturation = knob(0.5)),
        std::sync::Arc::new(LookModelSettings::default()),
    );
    let look = LookParams::resolve(&controls, &Primaries::REC_2020).unwrap();
    assert!(!look.is_neutral());
    let source = [0.5, 0.2, 0.1];
    let out = look.apply_to_pixel(source);
    assert!(lch(out).1 > lch(source).1 * 1.4);
}
