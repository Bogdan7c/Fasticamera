//! Виды работ стенда: цена вида строится из рецепта продукта, паритет — из нейтральных настроек плюс одна ветка.

use camera_contract::lens_shading::LensShadingCalibrations;
use look_contract::{LookKnob, LookRecipe, LookSettings, ToneBaseModel};
use render_loop::config::RenderLoopConfig;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::saturation_bench::inputs::{InputSources, gather_inputs};

/// Входы без движка и без библиотеки пресетов: запасной сенсор, нейтральный вид продукта на основе из конфига.
fn inputs() -> ProductInputs {
    let render = RenderLoopConfig::default();
    let screen = CameraScreenConfig::default();
    let lens_shading = LensShadingCalibrations::none();
    gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &lens_shading,
        render_config: &render,
        look: None,
        screen: &screen,
    })
}

fn parity(base: ToneBaseModel, case: ParityCase) -> ParityLook {
    ParityLook { base, case }
}

#[test]
fn product_case_keeps_the_product_settings_and_only_changes_the_base() {
    let inputs = inputs();
    let recipe = parity(ToneBaseModel::Filmic, ParityCase::ProductLook)
        .recipe(&inputs)
        .unwrap();
    assert_eq!(recipe.base, ToneBaseModel::Filmic);
    assert_eq!(recipe.settings, inputs.product_look().settings);
}

#[test]
fn each_branch_case_turns_on_exactly_its_own_branch_over_neutral_settings() {
    let inputs = inputs();
    let neutral = LookSettings::NEUTRAL;

    let bands = parity(ToneBaseModel::Agx, ParityCase::BandExtremes)
        .recipe(&inputs)
        .unwrap();
    assert!(!bands.settings.bands.is_neutral(), "полосы тронуты");
    assert_eq!(bands.settings.wheels, neutral.wheels, "колёса не тронуты");
    assert_eq!(bands.settings.contrast, neutral.contrast, "тон не тронут");

    let wheels = parity(ToneBaseModel::Aces, ParityCase::WheelsSample)
        .recipe(&inputs)
        .unwrap();
    assert_eq!(wheels.settings.wheels, inputs.measurement_look().wheels_sample);
    assert!(wheels.settings.bands.is_neutral() && wheels.settings.contrast == neutral.contrast);

    let tone = parity(ToneBaseModel::Legacy, ParityCase::ToneSample)
        .recipe(&inputs)
        .unwrap();
    let sample = inputs.measurement_look().tone_sample;
    assert_eq!(
        (
            tone.settings.contrast,
            tone.settings.midtones,
            tone.settings.shadows,
            tone.settings.highlights
        ),
        (sample.contrast, sample.midtones, sample.shadows, sample.highlights)
    );
    assert_eq!(tone.base, ToneBaseModel::Legacy);
    assert_eq!(tone.settings.wheels, neutral.wheels);
}

#[test]
fn band_extremes_push_the_luminance_of_every_band_of_the_model_to_the_minus_one_edge() {
    let inputs = inputs();
    let recipe = parity(ToneBaseModel::Agx, ParityCase::BandExtremes)
        .recipe(&inputs)
        .unwrap();
    let adjustments: Vec<_> = recipe.settings.bands.iter().collect();
    assert_eq!(
        adjustments.len(),
        inputs.look_model().bands.len(),
        "правка у каждой полосы модели"
    );
    for adjustment in adjustments {
        assert_eq!(adjustment.hue, LookKnob::NEUTRAL);
        assert_eq!(adjustment.saturation, LookKnob::NEUTRAL);
        assert_eq!(adjustment.luminance, LookKnob::new(-1.0).unwrap());
    }
}

#[test]
fn measured_look_serializes_as_the_stable_measurement_name() {
    let looks = [
        (MeasurementLook::Product, "product"),
        (MeasurementLook::NeutralOnProductBase, "neutral"),
        (MeasurementLook::ToneBase(ToneBaseModel::Legacy), "tone_base_legacy"),
        (MeasurementLook::WithoutTone, "without_tone"),
    ];
    for (look, name) in looks {
        assert_eq!(
            serde_json::to_value(MeasuredLook(look)).unwrap(),
            serde_json::json!(name)
        );
    }
}

#[test]
fn measured_look_recipe_is_built_from_the_product_recipe() {
    let inputs = inputs();
    let neutral = MeasuredLook(MeasurementLook::NeutralOnProductBase).recipe(&inputs);
    assert_eq!(
        neutral,
        LookRecipe {
            base: inputs.product_look().base,
            settings: LookSettings::NEUTRAL
        }
    );
    let sample = MeasuredLook(MeasurementLook::WheelsSample).recipe(&inputs);
    assert_eq!(sample.settings.wheels, inputs.measurement_look().wheels_sample);
}

/// Допуск паритета выбирается по рецепту (14o): только рецепт с тронутыми полосами получает именованный допуск
/// усиления у почти серых пикселей; колёса, тон и нейтральный вид продукта меряются на стандартном 1e-5 — превышение
/// там осталось бы дефектом, а не «разрешённым шумом».
#[test]
fn only_recipes_with_touched_bands_get_the_near_neutral_bands_allowance() {
    use crate::saturation_bench::look_parity_run::allowance_for;
    use isp_vulkan::saturation::parity::{ErrorAllowance, MATH_EXCESS_TOLERANCE};

    let inputs = inputs();
    for base in [
        ToneBaseModel::Legacy,
        ToneBaseModel::Agx,
        ToneBaseModel::Aces,
        ToneBaseModel::Filmic,
    ] {
        let allowance_of = |case| allowance_for(&parity(base, case).recipe(&inputs).unwrap());
        assert_eq!(
            allowance_of(ParityCase::BandExtremes),
            ErrorAllowance::NearNeutralBandsAmplification,
            "{base:?}: полосы"
        );
        for narrow in [
            ParityCase::WheelsSample,
            ParityCase::ToneSample,
            ParityCase::ProductLook,
        ] {
            assert_eq!(allowance_of(narrow), ErrorAllowance::Standard, "{base:?}: {narrow:?}");
        }
    }
    // Пресет владельца с тронутой полосой — тот же случай усиления: допуск решает рецепт, а не имя случая.
    let mut banded_product = parity(ToneBaseModel::Agx, ParityCase::BandExtremes)
        .recipe(&inputs)
        .unwrap();
    banded_product.settings.contrast = LookKnob::new(0.2).unwrap();
    assert_eq!(
        allowance_for(&banded_product),
        ErrorAllowance::NearNeutralBandsAmplification
    );
    // Именованный допуск шире стандартного, но не «выключает» проверку: 3e-5 против 1e-5.
    assert!(ErrorAllowance::NearNeutralBandsAmplification.math_excess() > MATH_EXCESS_TOLERANCE);
    assert!(ErrorAllowance::NearNeutralBandsAmplification.math_excess() <= 3.0 * MATH_EXCESS_TOLERANCE);
}
