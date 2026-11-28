//! Вид и основа в отчётах сценариев (14o, решения 312–316): что на самом деле показывал рендер, а не что просили.
//!
//! Один тип на отчёты `viewfinder-stage-budget` и `viewfinder`: разбор в `scripts/` падает на отчёте без этих
//! полей. Строится из диагностики видоискателя (`ViewfinderDiagnostics::look`, `tone_base`) — то есть из того, что
//! рендер применил, — плюс имя вида замера, если его ставил сценарий.

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};
use render_loop::viewfinder::ViewfinderDiagnostics;
use serde::Serialize;

use crate::measurement_look::{MeasurementLook, tone_knobs_active};

/// Имя вида, когда сценарий вид не ставил: рендер показывает вид продукта.
pub const PRODUCT_LOOK_NAME: &str = "product";

/// Вид и модель основы, как их показывал рендер.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LookReport {
    /// Модель основы (`agx`, `aces`, `filmic`, `legacy`).
    pub tone_base: &'static str,
    /// Отпечаток рецепта (основа + ручки), 16 шестнадцатеричных цифр: тот же, что в логе `look_recipe`.
    pub recipe_fingerprint: String,
    /// Слой вида целиком в нуле (ни тона, ни цвета, ни полос, ни колёс).
    pub layer_neutral: bool,
    /// Колёса цвета не в нуле (шейдер колёс исполняется).
    pub wheels_active: bool,
    /// Хоть одна ручка тона не в нуле (стадия тона 14l исполняется).
    pub tone_active: bool,
    /// Хоть одна из 12 полосок не в нуле (стадия полосок 14t исполняется).
    pub primaries_active: bool,
    /// Имя вида замера (`tone_base_agx`, `neutral`, …), если его ставил сценарий; иначе `product`.
    pub measurement_look: &'static str,
}

impl LookReport {
    /// Отчёт по диагностике видоискателя; `measurement` — вид, который сценарий поставил в это окно (`None` —
    /// сценарий вида не ставил).
    pub fn of(viewfinder: &ViewfinderDiagnostics, measurement: Option<MeasurementLook>) -> Self {
        Self::of_parts(viewfinder.tone_base, &viewfinder.look, measurement)
    }

    /// Отчёт по основе и значениям, которые рендер показывал (так окно собирает его из своего показания).
    pub fn of_parts(base: ToneBaseModel, settings: &LookSettings, measurement: Option<MeasurementLook>) -> Self {
        let recipe = LookRecipe {
            base,
            settings: settings.clone(),
        };
        Self {
            tone_base: base.name(),
            recipe_fingerprint: recipe.fingerprint_hex(),
            layer_neutral: settings.is_neutral(),
            wheels_active: !settings.wheels.is_neutral(),
            tone_active: tone_knobs_active(settings),
            primaries_active: !settings.primaries.is_neutral(),
            measurement_look: measurement.map_or(PRODUCT_LOOK_NAME, MeasurementLook::name),
        }
    }
}

#[cfg(test)]
#[path = "look_report_tests.rs"]
mod tests;
