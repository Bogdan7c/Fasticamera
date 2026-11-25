//! Конфиг детектора смены сцены: значения по умолчанию верны, каждая проверка называет своё поле.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use super::*;

/// Поле, которое назвала ошибка проверки.
fn rejected_field(config: SceneChangeConfig) -> &'static str {
    match config.validate().expect_err("конфиг должен быть отвергнут") {
        ConfigError::Invalid { field, .. } => field,
    }
}

#[test]
fn defaults_are_the_documented_numbers_valid_and_round_trip_through_json() {
    let config = SceneChangeConfig::default();
    config.validate().unwrap();
    // Решение 241: порог ячейки 0,5 EV, доля 0,4 (подбор на RAW-кадрах), подтверждение 500 мс.
    assert_eq!(config.cell_threshold_ev, 0.5);
    assert_eq!(config.changed_share, 0.4);
    assert_eq!(config.confirm_ms, 500);
    assert_eq!(config.clipped_cell_share, 0.01);
    let json = serde_json::to_string(&config).unwrap();
    assert_eq!(serde_json::from_str::<SceneChangeConfig>(&json).unwrap(), config);
}

#[test]
fn unknown_fields_are_rejected_and_a_partial_config_keeps_the_defaults() {
    assert!(serde_json::from_str::<SceneChangeConfig>(r#"{"nonsense": 1}"#).is_err());
    let partial: SceneChangeConfig = serde_json::from_str(r#"{"confirm_ms": 800}"#).unwrap();
    assert_eq!(partial.confirm_ms, 800);
    assert_eq!(partial.changed_share, SceneChangeConfig::default().changed_share);
}

#[test]
fn every_bad_number_is_rejected_with_the_name_of_its_field() {
    let default = SceneChangeConfig::default();
    let cases: [(SceneChangeConfig, &str); 12] = [
        (
            SceneChangeConfig {
                grid_columns: 0,
                ..default
            },
            "scene_change.grid_columns",
        ),
        (
            SceneChangeConfig {
                grid_rows: 0,
                ..default
            },
            "scene_change.grid_rows",
        ),
        (
            SceneChangeConfig {
                confirm_frames: 0,
                ..default
            },
            "scene_change.confirm_frames",
        ),
        (
            SceneChangeConfig {
                confirm_ms: 0,
                ..default
            },
            "scene_change.confirm_ms",
        ),
        (
            SceneChangeConfig {
                cell_threshold_ev: 0.0,
                ..default
            },
            "scene_change.cell_threshold_ev",
        ),
        (
            SceneChangeConfig {
                cell_threshold_ev: f32::NAN,
                ..default
            },
            "scene_change.cell_threshold_ev",
        ),
        (
            SceneChangeConfig {
                changed_share: 0.0,
                ..default
            },
            "scene_change.changed_share",
        ),
        (
            SceneChangeConfig {
                changed_share: 1.5,
                ..default
            },
            "scene_change.changed_share",
        ),
        (
            SceneChangeConfig {
                clipped_cell_share: 0.0,
                ..default
            },
            "scene_change.clipped_cell_share",
        ),
        (
            SceneChangeConfig {
                dark_green_level: -1.0,
                ..default
            },
            "scene_change.dark_green_level",
        ),
        (
            SceneChangeConfig {
                min_voting_cells: 0,
                ..default
            },
            "scene_change.min_voting_cells",
        ),
        // Больше ячеек, чем в сетке 8×6: вывода не будет никогда.
        (
            SceneChangeConfig {
                min_voting_cells: 49,
                ..default
            },
            "scene_change.min_voting_cells",
        ),
    ];
    for (config, expected_field) in cases {
        assert_eq!(rejected_field(config), expected_field, "{config:?}");
    }
}

#[test]
fn a_detector_refuses_a_bad_config() {
    let bad = SceneChangeConfig {
        confirm_frames: 0,
        ..SceneChangeConfig::default()
    };
    assert!(super::super::scene_change::SceneChangeDetector::new(bad).is_err());
}
