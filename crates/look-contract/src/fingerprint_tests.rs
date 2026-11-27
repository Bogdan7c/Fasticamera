//! Тесты отпечатка рецепта: равные рецепты совпадают, любое одиночное изменение меняет отпечаток.

use super::*;
use crate::{BandAdjustment, BandAdjustments, ColorWheel, GlobalKnob, HueBandId, LookKnob, LookSettings, WheelPush};

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).unwrap()
}

fn band_adjustment(name: &str, hue: f32) -> BandAdjustment {
    BandAdjustment {
        band: HueBandId::new(name).unwrap(),
        hue: knob(hue),
        saturation: knob(0.2),
        luminance: knob(-0.1),
    }
}

/// Рецепт, где ненулевое всё: каждое поле влияет на отпечаток.
fn rich_recipe() -> LookRecipe {
    let mut settings = LookSettings::NEUTRAL;
    for (index, id) in GlobalKnob::ALL.into_iter().enumerate() {
        settings = settings.with_knob(id, knob(0.05 * (index as f32 + 1.0)));
    }
    settings.bands = BandAdjustments::new(vec![band_adjustment("sky", 0.3), band_adjustment("green", -0.2)]).unwrap();
    for (index, zone) in WheelZone::ALL.into_iter().enumerate() {
        let wheel = ColorWheel {
            push: WheelPush::new(0.1 + 0.05 * index as f32, -0.1).unwrap(),
            ring: knob(0.02 * (index as f32 + 1.0)),
        };
        settings.wheels = settings.wheels.with_wheel(zone, wheel);
    }
    LookRecipe {
        base: ToneBaseModel::Aces,
        settings,
    }
}

#[test]
fn equal_recipes_have_equal_fingerprints_and_hex_is_16_digits() {
    let first = rich_recipe();
    let second = rich_recipe();
    assert_eq!(first.fingerprint(), second.fingerprint());
    let hex = first.fingerprint_hex();
    assert_eq!(hex.len(), 16);
    assert_eq!(u64::from_str_radix(&hex, 16).unwrap(), first.fingerprint());
}

#[test]
fn fingerprint_is_stable_across_releases() {
    // Закрепляет раскладку байтов: смена значения означает смену контракта логов (см. доку модуля).
    let neutral = LookRecipe {
        base: ToneBaseModel::Legacy,
        settings: LookSettings::NEUTRAL,
    };
    assert_eq!(neutral.fingerprint_hex(), "6686605a62c31778");
}

#[test]
fn every_base_gives_a_different_fingerprint() {
    let mut seen = std::collections::HashSet::new();
    for base in ToneBaseModel::ALL {
        let recipe = LookRecipe { base, ..rich_recipe() };
        assert!(seen.insert(recipe.fingerprint()), "основа {base:?} совпала с другой");
    }
}

#[test]
fn changing_any_single_global_knob_changes_the_fingerprint() {
    let recipe = rich_recipe();
    for id in GlobalKnob::ALL {
        let changed = LookRecipe {
            base: recipe.base,
            settings: recipe.settings.clone().with_knob(id, knob(-0.77)),
        };
        assert_ne!(
            changed.fingerprint(),
            recipe.fingerprint(),
            "ручка {id:?} не влияет на отпечаток"
        );
    }
}

#[test]
fn changing_any_band_value_or_the_band_set_changes_the_fingerprint() {
    let recipe = rich_recipe();
    let base_fp = recipe.fingerprint();
    let with_bands = |bands: Vec<BandAdjustment>| LookRecipe {
        base: recipe.base,
        settings: LookSettings {
            bands: BandAdjustments::new(bands).unwrap(),
            ..recipe.settings.clone()
        },
    };
    let sky = band_adjustment("sky", 0.3);
    let green = band_adjustment("green", -0.2);
    assert_ne!(
        with_bands(vec![band_adjustment("sky", 0.31), green.clone()]).fingerprint(),
        base_fp
    );
    assert_ne!(
        with_bands(vec![
            BandAdjustment {
                saturation: knob(0.5),
                ..sky.clone()
            },
            green.clone()
        ])
        .fingerprint(),
        base_fp
    );
    assert_ne!(
        with_bands(vec![
            BandAdjustment {
                luminance: knob(0.5),
                ..sky.clone()
            },
            green.clone()
        ])
        .fingerprint(),
        base_fp
    );
    assert_ne!(with_bands(vec![sky.clone()]).fingerprint(), base_fp, "пропавшая полоса");
    assert_ne!(
        with_bands(vec![band_adjustment("sky", 0.3), band_adjustment("foliage", -0.2)]).fingerprint(),
        base_fp,
        "другое имя полосы"
    );
}

#[test]
fn changing_any_wheel_part_changes_the_fingerprint() {
    let recipe = rich_recipe();
    for zone in WheelZone::ALL {
        let own = recipe.settings.wheels.wheel(zone);
        let variants = [
            ColorWheel {
                push: WheelPush::new(own.push.a() + 0.01, own.push.b()).unwrap(),
                ..own
            },
            ColorWheel {
                push: WheelPush::new(own.push.a(), own.push.b() + 0.01).unwrap(),
                ..own
            },
            ColorWheel {
                ring: knob(own.ring.get() + 0.01),
                ..own
            },
        ];
        for wheel in variants {
            let mut settings = recipe.settings.clone();
            settings.wheels = settings.wheels.with_wheel(zone, wheel);
            let changed = LookRecipe {
                base: recipe.base,
                settings,
            };
            assert_ne!(
                changed.fingerprint(),
                recipe.fingerprint(),
                "колесо {zone:?}: часть не влияет"
            );
        }
    }
}
