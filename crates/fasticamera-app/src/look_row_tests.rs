//! Тесты строк вида: имена уникальны, окна A и B различаются, рецепты окон действительно разные.

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};

use super::LookRow;
use crate::measurement_look::{MeasurementLook, MeasurementLookConfig};

/// Продукт с ненулевыми ручками тона и без колёс — на нём видна разница каждой строки.
fn product() -> LookRecipe {
    let neutral = LookRecipe {
        base: ToneBaseModel::Agx,
        settings: LookSettings::NEUTRAL,
    };
    MeasurementLook::ToneSample.recipe_from(&neutral, &MeasurementLookConfig::default())
}

#[test]
fn row_names_are_unique_and_prefixed() {
    let names: std::collections::HashSet<_> = LookRow::ALL.iter().map(|row| row.name()).collect();
    assert_eq!(names.len(), LookRow::ALL.len());
    assert!(names.iter().all(|name| name.starts_with("look_")));
}

#[test]
fn every_row_shows_different_recipes_in_window_a_and_b() {
    let config = MeasurementLookConfig::default();
    let product = product();
    for row in LookRow::ALL {
        let on = row.on_look().recipe_from(&product, &config);
        let off = row.off_look().recipe_from(&product, &config);
        assert_ne!(on, off, "строка {} не меняет рецепт между окнами", row.name());
        assert_ne!(on.fingerprint(), off.fingerprint(), "строка {}", row.name());
    }
}

#[test]
fn tone_base_rows_differ_only_in_base() {
    let config = MeasurementLookConfig::default();
    let product = product();
    for row in [LookRow::ToneBaseAgx, LookRow::ToneBaseAces, LookRow::ToneBaseFilmic] {
        let on = row.on_look().recipe_from(&product, &config);
        let off = row.off_look().recipe_from(&product, &config);
        assert_eq!(on.settings, off.settings, "строка {}: ручки те же", row.name());
        assert_eq!(off.base, ToneBaseModel::Legacy);
    }
}

#[test]
fn look_rows_are_known_to_the_config_by_name_and_allowed_on_every_surface() {
    use crate::stage_budget_surface_config::{StageBudgetSurface, row_allowed_on, validate_rows_for_surface};
    use crate::viewfinder_scenario_config::StageBudgetRowSetting;
    let all = StageBudgetRowSetting::all_rows();
    for row in LookRow::ALL {
        let setting = StageBudgetRowSetting::Look(row);
        assert!(all.contains(&setting), "{} нет в списке строк", row.name());
        assert_eq!(StageBudgetRowSetting::from_name(row.name()), Ok(setting));
        assert_eq!(serde_json::to_string(&setting).unwrap(), format!("\"{}\"", row.name()));
        assert_eq!(
            serde_json::from_str::<StageBudgetRowSetting>(&format!("\"{}\"", row.name())).unwrap(),
            setting
        );
        for surface in [
            StageBudgetSurface::BareViewfinder,
            StageBudgetSurface::PhotoScreen,
            StageBudgetSurface::CalibrateScreen,
        ] {
            assert!(row_allowed_on(setting, surface), "{} на {}", row.name(), surface.name());
        }
    }
    let rows: Vec<_> = LookRow::ALL.into_iter().map(StageBudgetRowSetting::Look).collect();
    assert!(validate_rows_for_surface(&rows, StageBudgetSurface::BareViewfinder).is_ok());
    assert!(
        !StageBudgetRowSetting::default_rows()
            .iter()
            .any(|row| matches!(row, StageBudgetRowSetting::Look(_))),
        "строки вида не входят в умолчание: меряются только по просьбе"
    );
}
