//! Тесты вида для замеров: каждый вариант меняет ровно одну вещь рецепта продукта, образцы проверяются.

use look_contract::{
    ChannelPrimaries, ColorWheels, LookKnob, LookRecipe, LookSettings, PrimaryChannel, ToneBaseModel, WheelZone,
};

use super::{MeasurementLook, MeasurementLookConfig, MeasurementLookConfigError, ToneKnobsSample, tone_knobs_active};

/// Рецепт «как у владельца»: контраст и цвет тронуты, колёс нет (как «Как глаз»), основа ACES.
fn product_recipe() -> LookRecipe {
    LookRecipe {
        base: ToneBaseModel::Aces,
        settings: LookSettings {
            contrast: LookKnob::new(0.02).unwrap(),
            saturation: LookKnob::new(0.08).unwrap(),
            vibrance: LookKnob::new(0.10).unwrap(),
            ..LookSettings::NEUTRAL
        },
    }
}

#[test]
fn default_config_is_valid_and_turns_both_shader_branches_on() {
    let config = MeasurementLookConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert!(
        !config.wheels_sample.is_neutral(),
        "образец колёс обязан включать ветку колёс"
    );
    assert!(
        !config.tone_sample.is_neutral(),
        "образец тона обязан включать стадию тона"
    );
    assert!(
        !config.primaries_sample.is_neutral(),
        "образец полосок обязан включать стадию полосок"
    );
}

#[test]
fn neutral_samples_are_rejected_with_the_field_named() {
    let neutral_wheels = MeasurementLookConfig {
        wheels_sample: ColorWheels::NEUTRAL,
        ..MeasurementLookConfig::default()
    };
    assert_eq!(
        neutral_wheels.validate(),
        Err(MeasurementLookConfigError::NeutralWheelsSample)
    );
    let neutral_tone = MeasurementLookConfig {
        tone_sample: ToneKnobsSample::NEUTRAL,
        ..MeasurementLookConfig::default()
    };
    assert_eq!(
        neutral_tone.validate(),
        Err(MeasurementLookConfigError::NeutralToneSample)
    );
    let neutral_primaries = MeasurementLookConfig {
        primaries_sample: ChannelPrimaries::NEUTRAL,
        ..MeasurementLookConfig::default()
    };
    assert_eq!(
        neutral_primaries.validate(),
        Err(MeasurementLookConfigError::NeutralPrimariesSample)
    );
}

#[test]
fn primaries_on_and_off_differ_only_in_the_twelve_primaries() {
    let product = product_recipe();
    let config = MeasurementLookConfig::default();
    let on = MeasurementLook::PrimariesSample.recipe_from(&product, &config);
    let off = MeasurementLook::WithoutPrimaries.recipe_from(&product, &config);
    assert_eq!(on.settings.primaries, config.primaries_sample);
    assert!(off.settings.primaries.is_neutral());
    // Всё, кроме полосок, — как у продукта: разница окон = цена полосок.
    assert_eq!(
        LookSettings {
            primaries: ChannelPrimaries::NEUTRAL,
            ..on.settings.clone()
        },
        off.settings
    );
    assert_eq!(
        off.settings, product.settings,
        "у продукта полосок нет — «без полосок» совпадает с ним"
    );
    assert_eq!((on.base, off.base), (product.base, product.base));
    assert_ne!(on.fingerprint(), off.fingerprint(), "отпечаток рецепта видит полоски");
}

#[test]
fn product_is_returned_unchanged() {
    let product = product_recipe();
    let config = MeasurementLookConfig::default();
    assert_eq!(MeasurementLook::Product.recipe_from(&product, &config), product);
}

#[test]
fn neutral_keeps_the_product_base_and_zeroes_the_whole_layer() {
    let recipe =
        MeasurementLook::NeutralOnProductBase.recipe_from(&product_recipe(), &MeasurementLookConfig::default());
    assert_eq!(recipe.base, ToneBaseModel::Aces, "основа продукта не меняется");
    assert!(recipe.settings.is_neutral());
}

#[test]
fn tone_base_changes_only_the_base() {
    let product = product_recipe();
    let recipe =
        MeasurementLook::ToneBase(ToneBaseModel::Legacy).recipe_from(&product, &MeasurementLookConfig::default());
    assert_eq!(recipe.base, ToneBaseModel::Legacy);
    assert_eq!(recipe.settings, product.settings, "ручки продукта не меняются");
}

#[test]
fn wheels_on_and_off_differ_only_in_wheels() {
    let product = product_recipe();
    let config = MeasurementLookConfig::default();
    let on = MeasurementLook::WheelsSample.recipe_from(&product, &config);
    let off = MeasurementLook::WithoutWheels.recipe_from(&product, &config);
    assert_eq!(on.settings.wheels, config.wheels_sample);
    assert!(off.settings.wheels.is_neutral());
    // Всё, кроме колёс, — как у продукта: разница окон = цена колёс.
    assert_eq!(
        LookSettings {
            wheels: ColorWheels::NEUTRAL,
            ..on.settings
        },
        off.settings
    );
    assert_eq!(
        off.settings, product.settings,
        "у продукта колёс нет — «без колёс» совпадает с ним"
    );
    assert_eq!((on.base, off.base), (product.base, product.base));
}

#[test]
fn tone_on_and_off_differ_only_in_the_four_tone_knobs() {
    let product = product_recipe();
    let config = MeasurementLookConfig::default();
    let on = MeasurementLook::ToneSample.recipe_from(&product, &config);
    let off = MeasurementLook::WithoutTone.recipe_from(&product, &config);
    assert!(tone_knobs_active(&on.settings));
    assert!(!tone_knobs_active(&off.settings), "без тона все четыре ручки в нуле");
    assert!(
        tone_knobs_active(&product.settings),
        "у продукта контраст 0,02 — стадия тона включена"
    );
    // Цвет продукта сохраняется в обоих окнах.
    assert_eq!(on.settings.saturation, product.settings.saturation);
    assert_eq!(off.settings.vibrance, product.settings.vibrance);
    assert_eq!(on.settings.contrast, config.tone_sample.contrast);
    assert_eq!(off.settings.contrast, LookKnob::NEUTRAL);
}

#[test]
fn names_are_distinct_and_stable() {
    let looks = [
        MeasurementLook::Product,
        MeasurementLook::NeutralOnProductBase,
        MeasurementLook::ToneBase(ToneBaseModel::Agx),
        MeasurementLook::ToneBase(ToneBaseModel::Aces),
        MeasurementLook::ToneBase(ToneBaseModel::Filmic),
        MeasurementLook::ToneBase(ToneBaseModel::Legacy),
        MeasurementLook::WheelsSample,
        MeasurementLook::WithoutWheels,
        MeasurementLook::ToneSample,
        MeasurementLook::WithoutTone,
        MeasurementLook::PrimariesSample,
        MeasurementLook::WithoutPrimaries,
    ];
    let mut names: Vec<_> = looks.iter().map(|look| look.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        looks.len(),
        "имена читает разбор в scripts/ — они обязаны различаться"
    );
    assert_eq!(MeasurementLook::WheelsSample.name(), "wheels_sample");
    assert_eq!(MeasurementLook::PrimariesSample.name(), "primaries_sample");
    assert_eq!(MeasurementLook::WithoutPrimaries.name(), "without_primaries");
}

#[test]
fn config_round_trips_and_rejects_unknown_fields() {
    let config = MeasurementLookConfig::default();
    let text = serde_json::to_string(&config).unwrap();
    let back: MeasurementLookConfig = serde_json::from_str(&text).unwrap();
    assert_eq!(back, config);
    let unknown = serde_json::from_str::<MeasurementLookConfig>(r#"{"wheel_sample": {}}"#);
    assert!(
        unknown.is_err(),
        "опечатка в имени поля — ошибка, а не молчаливое умолчание"
    );
}

#[test]
fn every_default_sample_literal_survives_its_checks() {
    // `knob`/`default_wheels_sample` не паникуют: сломанный литерал стал бы нулём. Этот тест ловит ЛЮБОЙ такой литерал,
    // а не только случай «все нейтральны», который ловит `validate`.
    let config = MeasurementLookConfig::default();
    let tone = config.tone_sample;
    assert_eq!(
        [
            tone.contrast.get(),
            tone.midtones.get(),
            tone.shadows.get(),
            tone.highlights.get()
        ],
        [0.20, 0.10, 0.15, -0.15]
    );
    let wheels = config.wheels_sample;
    let pushes = [wheels.lift, wheels.gamma, wheels.gain, wheels.offset].map(|wheel| [wheel.push.a(), wheel.push.b()]);
    assert_eq!(pushes, [[0.10, -0.20], [-0.15, 0.10], [0.20, 0.15], [-0.05, -0.10]]);
    let rings = [wheels.lift, wheels.gamma, wheels.gain, wheels.offset].map(|wheel| wheel.ring.get());
    assert_eq!(rings, [0.05, -0.05, 0.05, 0.0]);
    // Образец полосок: все 12 ручек тронуты, по зонам Lift, Gamma, Gain, Offset и каналам R, G, B.
    let primaries = config.primaries_sample;
    let by_zone = WheelZone::ALL.map(|zone| PrimaryChannel::ALL.map(|channel| primaries.value(zone, channel).get()));
    assert_eq!(
        by_zone,
        [
            [0.10, -0.05, 0.08],
            [-0.10, 0.08, -0.06],
            [0.06, -0.08, 0.10],
            [-0.05, 0.04, -0.06]
        ]
    );
}
