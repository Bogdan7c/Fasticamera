use super::*;
use crate::runtime_config::RuntimeConfig;
use crate::viewfinder_scenario_config::{IspStageSetting, ViewfinderScenarioConfig};

const ALL_SURFACES: [StageBudgetSurface; 3] = [
    StageBudgetSurface::BareViewfinder,
    StageBudgetSurface::PhotoScreen,
    StageBudgetSurface::CalibrateScreen,
];

/// Допустимость каждой строки на каждом месте — по таблице брифа 16a (все 6 новых и старых сочетаний).
#[test]
fn every_row_is_allowed_exactly_where_the_table_says() {
    use StageBudgetSurface::{BareViewfinder, CalibrateScreen, PhotoScreen};
    let table: [(StageBudgetRowSetting, [bool; 3]); 6] = [
        (StageBudgetRowSetting::Stage(IspStageSetting::Tone), [true, true, true]),
        (StageBudgetRowSetting::WholeViewfinder, [true, true, true]),
        (StageBudgetRowSetting::ColorScopes, [false, false, true]),
        (StageBudgetRowSetting::UiOverlay, [false, true, true]),
        (StageBudgetRowSetting::WholeProduct, [false, true, true]),
        (StageBudgetRowSetting::Display120Hz, [true, true, true]),
    ];
    for (row, expected) in table {
        for (surface, want) in [BareViewfinder, PhotoScreen, CalibrateScreen].into_iter().zip(expected) {
            assert_eq!(
                row_allowed_on(row, surface),
                want,
                "строка {} на месте {}",
                row.name(),
                surface.name()
            );
        }
    }
}

#[test]
fn a_disallowed_row_is_a_typed_error_naming_row_surface_and_allowed_places() {
    let error = validate_rows_for_surface(
        &[StageBudgetRowSetting::Display120Hz, StageBudgetRowSetting::ColorScopes],
        StageBudgetSurface::PhotoScreen,
    )
    .expect_err("приборов на экране PHOTO нет");
    assert_eq!(
        error,
        ViewfinderScenarioConfigError::RowNotAllowedOnSurface {
            row: "color_scopes",
            surface: "photo_screen",
            allowed: "calibrate_screen",
        }
    );
    let text = error.to_string();
    assert!(text.contains("color_scopes") && text.contains("photo_screen"), "{text}");
}

#[test]
fn default_surface_is_the_bare_viewfinder_and_default_rows_stay_valid_on_it() {
    let config = ViewfinderScenarioConfig::default();
    assert_eq!(config.stage_budget.surface, StageBudgetSurface::BareViewfinder);
    config.validate().expect("умолчания верны");
    assert!(!StageBudgetSurface::BareViewfinder.keeps_product_screen());
    assert!(StageBudgetSurface::PhotoScreen.keeps_product_screen());
    assert!(StageBudgetSurface::CalibrateScreen.runs_color_scopes());
    assert!(!StageBudgetSurface::PhotoScreen.runs_color_scopes());
    for surface in ALL_SURFACES {
        assert_eq!(surface.draws_ui_overlay(), surface.keeps_product_screen());
    }
}

fn parse(json: &str) -> Result<RuntimeConfig, crate::runtime_config::ConfigError> {
    RuntimeConfig::from_json(json)
}

#[test]
fn surface_and_new_rows_come_from_json_and_unknown_names_are_errors() {
    let good = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{
            "surface":"calibrate_screen","rows":["color_scopes","ui_overlay","whole_product","display_120hz"]}}}}}"#;
    let runtime = parse(good).expect("конфиг разобран");
    let budget = &runtime.camera().scenarios.viewfinder.stage_budget;
    assert_eq!(budget.surface, StageBudgetSurface::CalibrateScreen);
    assert_eq!(
        budget.rows,
        [
            StageBudgetRowSetting::ColorScopes,
            StageBudgetRowSetting::UiOverlay,
            StageBudgetRowSetting::WholeProduct,
            StageBudgetRowSetting::Display120Hz
        ]
    );

    let unknown_surface = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"surface":"video_screen"}}}}}"#;
    assert!(parse(unknown_surface).is_err(), "неизвестное место — ошибка разбора");
    let unknown_row = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{"rows":["color_scope"]}}}}}"#;
    assert!(parse(unknown_row).is_err(), "неизвестная строка — ошибка разбора");
}

#[test]
fn runtime_config_rejects_a_row_that_does_not_fit_the_surface_before_any_measurement() {
    let bad = r#"{"build_type":"debug","log_filter":"info",
        "camera":{"scenarios":{"viewfinder":{"stage_budget":{
            "surface":"bare_viewfinder","rows":["ui_overlay"]}}}}}"#;
    let error = parse(bad).expect_err("UI на голом видоискателе не бывает");
    assert!(error.to_string().contains("ui_overlay"), "{error}");
}

#[test]
fn hold_point_tolerance_and_timeout_are_validated() {
    let mut config = ViewfinderScenarioConfig::default();
    config.stage_budget.display_hold_point.x = 1.5;
    assert!(matches!(
        config.validate(),
        Err(ViewfinderScenarioConfigError::ScreenPointOutOfRange { .. })
    ));
    let mut config = ViewfinderScenarioConfig::default();
    config.stage_budget.display_refresh_tolerance_hz = 0.0;
    assert!(matches!(
        config.validate(),
        Err(ViewfinderScenarioConfigError::NotPositive { .. })
    ));
    let mut config = ViewfinderScenarioConfig::default();
    config.stage_budget.surface_ready_timeout_ms = 0;
    assert!(matches!(
        config.validate(),
        Err(ViewfinderScenarioConfigError::Zero { .. })
    ));
}
