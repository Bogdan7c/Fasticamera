//! Паритет вида на устройстве (сессия 14o): один случай [`ParityLook`] на худших синтетических сценах против CPU-эталона.
//!
//! Что делает: строит рецепт и параметры случая (`ProductInputs::parity_params_with_look`), калибровку сцен от сенсора
//! и нейтрали продукта, гонит все сцены через линейку `isp_vulkan::saturation::parity::ParityRuler` и сворачивает
//! худшее в [`ParityOutcome`]. Любая ошибка (параметры, GPU, эталон) — `Unavailable` с причиной, а не «прошло».
//!
//! Допуск выбирает [`allowance_for`] по рецепту: стандартный (1e-5), а у рецепта с тронутыми полосами оттенка —
//! именованный `NearNeutralBandsAmplification` (3e-5; причина доказана в 14o, см. его описание). Особых допусков 14l
//! (`SteepToneAtWhite`, `LegacySrgbExtremeColour`) случаи стенда не требуют: крутых углов тона и sRGB-выхода здесь нет.

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_vulkan::saturation::parity::{
    ErrorAllowance, ParityRuler, SceneCalibration, bright_saturated_scene, look_scenes,
};
use look_contract::LookRecipe;

use super::inputs::ProductInputs;
use super::look_variant::ParityLook;
use super::parity_report::{ParityAccumulator, ParityOutcome};

/// Измеряет паритет случая `look` на сценах паритета вида плюс «яркие насыщенные» (худший случай тона).
pub fn measure_look_parity(context: &GpuContext, inputs: &ProductInputs, look: ParityLook) -> ParityOutcome {
    match look.recipe(inputs) {
        Ok(recipe) => measure_look_recipe_parity(context, inputs, &recipe),
        Err(reason) => ParityOutcome::Unavailable { reason },
    }
}

/// То же для произвольного рецепта вида (подбор и разбор случаев паритета).
pub fn measure_look_recipe_parity(context: &GpuContext, inputs: &ProductInputs, recipe: &LookRecipe) -> ParityOutcome {
    match try_measure(context, inputs, recipe) {
        Ok(outcome) => outcome,
        Err(reason) => ParityOutcome::Unavailable { reason },
    }
}

fn try_measure(context: &GpuContext, inputs: &ProductInputs, recipe: &LookRecipe) -> Result<ParityOutcome, String> {
    let params = inputs
        .parity_params_with_look(recipe)
        .map_err(|error| format!("параметры линейки: {error}"))?;
    let calibration = scene_calibration(inputs);
    let mut scenes = look_scenes(&calibration);
    scenes.push(("bright_saturated", bright_saturated_scene(&calibration)));
    let mut ruler = ParityRuler::new(context).map_err(|error| format!("линейка не создана: {error}"))?;
    let mut accumulator = ParityAccumulator::new(allowance_for(recipe));
    for (scene, frame) in &scenes {
        let measure = ruler
            .measure(frame, &params, DisplayColorSpace::DisplayP3)
            .map_err(|error| format!("сцена {scene}: {error}"))?;
        accumulator.add_scene(scene, &measure);
    }
    Ok(accumulator.finish())
}

/// Допуск ошибки математики для рецепта `recipe`.
///
/// Решает рецепт, а не имя случая: тронутые полосы оттенка у ЛЮБОГО случая (и у пресета владельца в `ProductLook`)
/// включают ветку, где вес «не серый» раздувает ULP-разницу основы у почти серых пикселей, — значит и допуск их.
/// Колёса, тон и нейтральный вид остаются на стандартном: у них такого усиления нет, и превышение там — дефект.
pub fn allowance_for(recipe: &LookRecipe) -> ErrorAllowance {
    if recipe.settings.bands.is_neutral() {
        ErrorAllowance::Standard
    } else {
        ErrorAllowance::NearNeutralBandsAmplification
    }
}

/// Калибровка сцен от сенсора и нейтрали продукта: серый объект остаётся серым после баланса белого именно этого
/// телефона, уровни чёрного и белого — его.
fn scene_calibration(inputs: &ProductInputs) -> SceneCalibration {
    let levels = inputs.sensor.static_levels();
    SceneCalibration {
        pattern: inputs.sensor.pattern(),
        black: levels.black_pattern(),
        white: levels.white(),
        neutral: inputs.neutral.values(),
    }
}
