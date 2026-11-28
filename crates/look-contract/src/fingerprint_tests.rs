//! Тесты отпечатка рецепта: равные рецепты совпадают, любое одиночное изменение меняет отпечаток.

use super::*;
use crate::{
    BandAdjustment, BandAdjustments, ChannelPrimaries, ColorWheel, GlobalKnob, HueBandId, LookKnob, LookSettings,
    PrimaryChannel, WheelPush,
};

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
    for (zone_index, zone) in WheelZone::ALL.into_iter().enumerate() {
        for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
            let value = knob(0.03 * (3 * zone_index + channel_index) as f32 - 0.15);
            settings.primaries = settings.primaries.with_value(zone, channel, value);
        }
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
    // Значение схемы раскладки 2 (14t: версия 1 → 2 и 12 нулевых полосок в конце). Получено прогоном; до 14t
    // нейтраль «Legacy» давала "6686605a62c31778" — старые логи с версией раскладки 1 сверять только между собой.
    assert_eq!(neutral.fingerprint_hex(), "66407fee89cb1a23");
    // Порядок полосок закреплён отдельным значением: Lift/Red = 0.5, остальные полоски в нуле.
    let one_bar = LookRecipe {
        base: ToneBaseModel::Legacy,
        settings: LookSettings {
            primaries: ChannelPrimaries::NEUTRAL.with_value(WheelZone::Lift, PrimaryChannel::Red, knob(0.5)),
            ..LookSettings::NEUTRAL
        },
    };
    assert_eq!(one_bar.fingerprint_hex(), "2d67a0b4b954b75a");
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

#[test]
fn changing_any_single_primary_bar_changes_the_fingerprint() {
    let recipe = rich_recipe();
    let mut seen = std::collections::HashSet::new();
    seen.insert(recipe.fingerprint());
    for zone in WheelZone::ALL {
        for channel in PrimaryChannel::ALL {
            let own = recipe.settings.primaries.value(zone, channel);
            let mut settings = recipe.settings.clone();
            settings.primaries = settings.primaries.with_value(zone, channel, knob(own.get() + 0.01));
            let changed = LookRecipe {
                base: recipe.base,
                settings,
            };
            // Новый отпечаток отличается и от исходного, и от всех других одиночных правок (inserted == true).
            assert!(
                seen.insert(changed.fingerprint()),
                "полоска {zone:?}/{channel:?}: не влияет на отпечаток"
            );
        }
    }
}
