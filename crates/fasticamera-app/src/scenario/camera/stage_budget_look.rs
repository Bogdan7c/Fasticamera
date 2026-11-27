//! Вид окна замера `viewfinder-stage-budget` (14o): из состояния окна и вида продукта получается план — что показать
//! на рендере и к чему вернуть его по концу.
//!
//! Вид продукта берётся из библиотеки пресетов экрана (`CameraScreenService::look_handle`): активный пресет с
//! правками владельца, как есть. Без библиотеки (ПК-сценарии, тесты) — нейтраль на основе конфига по умолчанию, ровно
//! как у исполнителя вида. Библиотеку метод только ЧИТАЕТ: никаких намерений, никакого файла пресетов.
//! Образцы колёс и тона берутся из конфига `camera.scenarios.measurement_look`, а не из кода.

use look_contract::LookRecipe;

use crate::look_effects::unlibraried_recipe;
use crate::measurement_look::MeasurementLook;
use crate::scenario::observing::ScenarioContext;
use crate::viewfinder_control::ScenarioLook;

/// План вида одного окна.
#[derive(Debug, Clone, PartialEq)]
pub struct LookPlan {
    /// Какой вид меряется (имя уходит в отчёт).
    pub look: MeasurementLook,
    /// Что показать и к чему вернуть рендер.
    pub scenario_look: ScenarioLook,
}

impl LookPlan {
    /// Рецепт, который окно обязано увидеть на рендере.
    pub fn expected_recipe(&self) -> &LookRecipe {
        &self.scenario_look.shown
    }
}

/// Вид продукта сейчас (активный рецепт библиотеки; без библиотеки — нейтраль на основе по умолчанию).
pub fn product_recipe(context: &ScenarioContext<'_>) -> LookRecipe {
    context
        .camera_screen
        .look_handle()
        .map_or_else(unlibraried_recipe, |handle| handle.active_recipe())
}

/// План для вида `look` из текущего вида продукта.
pub fn plan_for(context: &ScenarioContext<'_>, look: MeasurementLook) -> LookPlan {
    let product = product_recipe(context);
    let shown = look.recipe_from(&product, &context.camera.scenarios().measurement_look);
    LookPlan {
        look,
        scenario_look: ScenarioLook {
            shown,
            restore_to: product,
        },
    }
}
