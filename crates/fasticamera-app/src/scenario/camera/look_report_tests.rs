//! Тесты отчёта о виде и основе: флаги слоёв, отпечаток и имя вида замера.

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};

use super::{LookReport, PRODUCT_LOOK_NAME};
use crate::measurement_look::{MeasurementLook, MeasurementLookConfig};

fn neutral_recipe(base: ToneBaseModel) -> LookRecipe {
    LookRecipe {
        base,
        settings: LookSettings::NEUTRAL,
    }
}

#[test]
fn neutral_look_reports_every_layer_as_inactive_and_names_the_product() {
    let report = LookReport::of_parts(ToneBaseModel::Aces, &LookSettings::NEUTRAL, None);
    assert_eq!(report.tone_base, "aces");
    assert!(report.layer_neutral);
    assert!(!report.wheels_active);
    assert!(!report.tone_active);
    assert_eq!(report.measurement_look, PRODUCT_LOOK_NAME);
    assert_eq!(
        report.recipe_fingerprint,
        neutral_recipe(ToneBaseModel::Aces).fingerprint_hex()
    );
}

#[test]
fn wheels_and_tone_samples_switch_exactly_their_own_flags_on() {
    let config = MeasurementLookConfig::default();
    let product = neutral_recipe(ToneBaseModel::Agx);

    let wheels = MeasurementLook::WheelsSample.recipe_from(&product, &config);
    let wheels_report = LookReport::of_parts(wheels.base, &wheels.settings, Some(MeasurementLook::WheelsSample));
    assert!(wheels_report.wheels_active && !wheels_report.tone_active && !wheels_report.layer_neutral);
    assert_eq!(wheels_report.measurement_look, "wheels_sample");

    let tone = MeasurementLook::ToneSample.recipe_from(&product, &config);
    let tone_report = LookReport::of_parts(tone.base, &tone.settings, Some(MeasurementLook::ToneSample));
    assert!(tone_report.tone_active && !tone_report.wheels_active && !tone_report.layer_neutral);
    assert_eq!(tone_report.measurement_look, "tone_sample");
}

#[test]
fn the_fingerprint_follows_both_base_and_values() {
    let config = MeasurementLookConfig::default();
    let product = neutral_recipe(ToneBaseModel::Agx);
    let other_base = MeasurementLook::ToneBase(ToneBaseModel::Filmic).recipe_from(&product, &config);
    let other_values = MeasurementLook::ToneSample.recipe_from(&product, &config);
    let reports = [
        LookReport::of_parts(product.base, &product.settings, None),
        LookReport::of_parts(other_base.base, &other_base.settings, None),
        LookReport::of_parts(other_values.base, &other_values.settings, None),
    ];
    let unique: std::collections::HashSet<_> = reports.iter().map(|report| report.recipe_fingerprint.clone()).collect();
    assert_eq!(unique.len(), 3, "основа и значения меняют отпечаток независимо");
    assert_eq!(reports[1].tone_base, "filmic");
}

#[test]
fn the_report_serializes_all_fields_the_python_checker_requires() {
    let json = serde_json::to_value(LookReport::of_parts(
        ToneBaseModel::Legacy,
        &LookSettings::NEUTRAL,
        None,
    ))
    .unwrap();
    for key in [
        "tone_base",
        "recipe_fingerprint",
        "layer_neutral",
        "wheels_active",
        "tone_active",
        "measurement_look",
    ] {
        assert!(json.get(key).is_some(), "в отчёте нет поля {key}: {json}");
    }
}
