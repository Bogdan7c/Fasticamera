use super::*;
use crate::look::bands::{LookBands, MAX_LOOK_BANDS, resolve_bands};
use crate::look::model::{HueBandDefinition, LookModelSettings};
use std::f32::consts::TAU;

/// Набор с множителем цветности `multiplier` у каждой полосы (для проверки разбиения единицы).
fn bands_with_uniform_multiplier(model: &LookModelSettings, multiplier: f32) -> LookBands {
    let mut bands = resolve_bands(model, &band_adjustment("red", 0.0, 1.0, 0.0))
        .bands
        .unwrap();
    for slot in 0..bands.count as usize {
        bands.chroma_multiplier[slot] = multiplier;
    }
    bands
}

#[test]
fn windows_are_a_partition_of_unity_on_the_whole_circle() {
    let model = LookModelSettings::default();
    let bands = bands_with_uniform_multiplier(&model, 1.7);
    for step in 0..3600 {
        let hue = step as f32 / 3600.0 * TAU;
        let effect = bands.effect_at(hue);
        assert!((effect.chroma_multiplier - 1.7).abs() < 1e-5, "{hue}: {effect:?}");
    }
}

#[test]
fn each_band_dominates_its_own_centre_and_is_flat_there() {
    let model = LookModelSettings::default();
    let adjustment = band_adjustment("green", 0.5, 0.5, 0.0);
    let bands = resolve_bands(&model, &adjustment).bands.unwrap();
    let green = bands
        .centers_radians
        .iter()
        .position(|center| (center.to_degrees() - 142.0).abs() < 1e-3)
        .unwrap();
    let centre = bands.centers_radians[green];
    let at_centre = bands.effect_at(centre);
    assert!((at_centre.chroma_multiplier - (1.0 + 0.5 * model.band_saturation.plus)).abs() < 1e-6);
    assert!((at_centre.hue_shift_radians - (0.5 * model.band_hue_degrees.plus).to_radians()).abs() < 1e-6);
    // C¹ на центре: сдвиг на 0,01 рад меняет значение на величину второго порядка.
    for offset in [-0.01_f32, 0.01] {
        let moved = bands.effect_at(centre + offset);
        assert!(
            (moved.chroma_multiplier - at_centre.chroma_multiplier).abs() < 6e-4,
            "{offset}: {moved:?}"
        );
    }
    // Соседний центр (оранжевый, 60°) нейтрален: полный эффект зелёной полосы там нулевой.
    let orange = bands.effect_at(60.0_f32.to_radians());
    assert_eq!(orange.chroma_multiplier, 1.0);
    assert_eq!(orange.hue_shift_radians, 0.0);
}

#[test]
fn effect_is_continuous_across_every_window_boundary_including_360_to_0() {
    let model = LookModelSettings::default();
    // Все полосы тронуты по-разному: ступенька в любой точке круга была бы видна.
    let adjustments = look_contract::BandAdjustments::new(
        model
            .bands
            .iter()
            .enumerate()
            .map(|(index, band)| look_contract::BandAdjustment {
                band: band.id.clone(),
                hue: knob(if index % 2 == 0 { 1.0 } else { -1.0 }),
                saturation: knob(if index % 3 == 0 { 1.0 } else { -1.0 }),
                luminance: knob(if index % 4 == 0 { 1.0 } else { -1.0 }),
            })
            .collect(),
    )
    .unwrap();
    let bands = resolve_bands(&model, &adjustments).bands.unwrap();
    // Полный размах между соседними полосами с правками разных знаков: от края «−» до края «+» своей ручки.
    let hue_swing = (model.band_hue_degrees.minus + model.band_hue_degrees.plus).to_radians();
    let chroma_swing = model.band_saturation.minus + model.band_saturation.plus;
    let lightness_swing = model.band_luminance.minus + model.band_luminance.plus;
    let steps = 7200;
    let mut previous = bands.effect_at(0.0);
    let mut worst: f32 = 0.0;
    for step in 1..=steps {
        let current = bands.effect_at((step % steps) as f32 / steps as f32 * TAU);
        worst = worst
            .max((current.hue_shift_radians - previous.hue_shift_radians).abs() / hue_swing)
            .max((current.chroma_multiplier - previous.chroma_multiplier).abs() / chroma_swing)
            .max((current.lightness_multiplier - previous.lightness_multiplier).abs() / lightness_swing);
        previous = current;
    }
    // Шаг 0,05° по кругу; склон приподнятого косинуса между центрами не круче π/2 размаха на длину окна
    // (наименьшее окно ≈ 30°) — порог в 3 раза выше этого склона, ступенька (≥ 0,1 размаха) его пробьёт.
    let allowed = std::f32::consts::FRAC_PI_2 * (TAU / steps as f32) / 30.0_f32.to_radians() * 3.0;
    assert!(
        worst < allowed,
        "наибольший скачок {worst} против допустимого {allowed}"
    );
}

#[test]
fn another_band_set_from_the_config_works_without_code_changes() {
    let model = LookModelSettings {
        bands: vec![
            HueBandDefinition {
                id: band_id("warm"),
                center_degrees: 50.0,
            },
            HueBandDefinition {
                id: band_id("cool"),
                center_degrees: 230.0,
            },
            HueBandDefinition {
                id: band_id("odd-one"),
                center_degrees: 330.0,
            },
        ],
        ..LookModelSettings::default()
    };
    model.validate().unwrap();
    let resolved = resolve_bands(&model, &band_adjustment("cool", 0.0, -1.0, 0.0));
    let bands = resolved.bands.unwrap();
    assert_eq!(bands.count, 3);
    let at = |degrees: f32| bands.effect_at(degrees.to_radians()).chroma_multiplier;
    assert!((at(230.0) - (1.0 - model.band_saturation.minus)).abs() < 1e-6);
    assert_eq!(at(50.0), 1.0);
    assert_eq!(at(330.0), 1.0);
    // Центры в произвольном порядке: результат тот же.
    let mut shuffled = model.clone();
    shuffled.bands.reverse();
    let again = resolve_bands(&shuffled, &band_adjustment("cool", 0.0, -1.0, 0.0))
        .bands
        .unwrap();
    assert_eq!(again, bands);
    assert!(
        bands.centers_radians[3..MAX_LOOK_BANDS]
            .iter()
            .all(|center| *center == 0.0)
    );
}

#[test]
fn unknown_band_ids_are_listed_and_do_not_turn_the_stage_on() {
    let model = LookModelSettings::default();
    let only_unknown = resolve_bands(&model, &band_adjustment("teal-ish", 1.0, 1.0, 1.0));
    assert!(
        only_unknown.bands.is_none(),
        "правка неизвестной полосы включила стадию"
    );
    assert_eq!(only_unknown.unknown_band_ids, vec![band_id("teal-ish")]);
    let neutral = resolve_bands(&model, &look_contract::BandAdjustments::NONE);
    assert!(neutral.bands.is_none() && neutral.unknown_band_ids.is_empty());
}
