//! Тесты конфига раздела калибровки: умолчания допустимы, плохие числа отвергаются с понятной причиной,
//! конфиг экрана читается из JSON и отвергает неизвестные поля.

use scope_contract::{MAX_SCOPE_IMAGE_SIDE_PX, VectorscopeAxes};

use super::{CalibrationConfig, CalibrationConfigError};
use crate::calibration::{CalibrationLayoutConfigError, KnobsSide, ZoneShape};
use crate::config::{ScreenConfig, ScreenConfigError};
use crate::mode_behaviour::CalibrationAvailability;

#[test]
fn defaults_are_valid_enabled_and_use_the_video_standard_axes() {
    let config = CalibrationConfig::default();

    assert_eq!(config.validate(), Ok(()));
    assert_eq!(config.availability(), CalibrationAvailability::Enabled);
    assert_eq!(config.vectorscope_axes, VectorscopeAxes::YCbCrBt709);
    assert_eq!(config.max_scope_image_side_px, MAX_SCOPE_IMAGE_SIDE_PX);
}

#[test]
fn knob_finger_path_per_unit_is_the_full_travel_over_twice_the_units() {
    let mut widgets = CalibrationConfig::default().widgets;
    // Умолчание (решение 251): −200…+200 на 400 pt пальца — 1 pt на единицу, риска через 20 pt.
    assert_eq!(widgets.knob_points_per_unit(), 1.0);
    assert_eq!(
        f64::from(widgets.knob_tick_every_steps) * widgets.knob_points_per_unit(),
        20.0
    );
    widgets.knob_full_travel_pt = 800.0;
    assert_eq!(
        widgets.knob_points_per_unit(),
        2.0,
        "вдвое длиннее ход — вдвое шире шаг"
    );
    widgets.knob_units_per_full_scale = 100;
    assert_eq!(
        widgets.knob_points_per_unit(),
        4.0,
        "вдвое меньше единиц — вдвое шире шаг"
    );
}

#[test]
fn a_disabled_flag_maps_to_the_disabled_availability() {
    let config = CalibrationConfig {
        enabled: false,
        ..CalibrationConfig::default()
    };

    assert_eq!(config.availability(), CalibrationAvailability::Disabled);
}

#[test]
fn fractions_outside_the_open_unit_interval_are_rejected() {
    for bad in [0.0, 1.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
        let mut config = CalibrationConfig::default();
        config.layout.landscape.knobs_width_fraction = bad;
        assert!(
            matches!(
                config.validate(),
                Err(CalibrationConfigError::Layout(
                    CalibrationLayoutConfigError::FractionOutOfRange {
                        field: "landscape.knobs_width_fraction",
                        ..
                    }
                ))
            ),
            "{bad}"
        );
    }
}

#[test]
fn bottom_row_heights_and_the_scope_limit_are_validated() {
    let mut config = CalibrationConfig::default();
    config.layout.portrait.bottom_row_height_pt = 0.0;
    assert!(matches!(
        config.validate(),
        Err(CalibrationConfigError::Layout(
            CalibrationLayoutConfigError::NonPositive {
                field: "portrait.bottom_row_height_pt",
                ..
            }
        ))
    ));
    for (name, set) in [
        ("scope_strip_height_pt", 0.0),
        ("scope_levels_gutter_pt", f64::NAN),
        ("side_button_inset_pt", -1.0),
    ] {
        let mut config = CalibrationConfig::default();
        match name {
            "scope_strip_height_pt" => config.layout.scope_strip_height_pt = set,
            "scope_levels_gutter_pt" => config.layout.scope_levels_gutter_pt = set,
            _ => config.layout.side_button_inset_pt = set,
        }
        assert!(
            matches!(
                config.validate(),
                Err(CalibrationConfigError::Layout(CalibrationLayoutConfigError::NonPositive { field, .. })) if field == name
            ),
            "{name}"
        );
    }

    for bad in [0, MAX_SCOPE_IMAGE_SIDE_PX + 1] {
        let config = CalibrationConfig {
            max_scope_image_side_px: bad,
            ..CalibrationConfig::default()
        };
        assert!(
            matches!(config.validate(), Err(CalibrationConfigError::ScopeSideLimitOutOfRange { value, .. }) if value == bad),
            "{bad}"
        );
    }
}

#[test]
fn the_screen_config_reports_a_bad_calibration_section_and_reads_it_from_json() {
    let mut config = ScreenConfig::default();
    config.calibration.layout.portrait.preview_height_fraction = 2.0;
    assert!(matches!(config.validate(), Err(ScreenConfigError::Calibration(_))));
    assert!(
        crate::ScreenState::new(&config).is_err(),
        "невалидный конфиг не даёт создать экран"
    );

    let parsed: ScreenConfig =
        serde_json::from_str(r#"{"calibration":{"enabled":false,"vectorscope_axes":"oklab_ab"}}"#).expect("JSON");
    assert!(!parsed.calibration.enabled);
    assert_eq!(parsed.calibration.vectorscope_axes, VectorscopeAxes::OklabAb);
    assert_eq!(
        parsed.calibration.layout,
        CalibrationConfig::default().layout,
        "остальное — умолчания"
    );
    assert!(serde_json::from_str::<ScreenConfig>(r#"{"calibration":{"unknown":1}}"#).is_err());
}

#[test]
fn widget_numbers_are_validated_with_the_name_of_the_bad_field() {
    use super::CalibrationWidgetsConfigError as Bad;
    let reject = |edit: &dyn Fn(&mut CalibrationConfig)| {
        let mut config = CalibrationConfig::default();
        edit(&mut config);
        config.validate()
    };
    assert!(matches!(
        reject(&|c| c.widgets.row_height_pt = f64::NAN),
        Err(CalibrationConfigError::Widgets(Bad::NonPositive {
            field: "row_height_pt",
            ..
        }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.knob_units_per_full_scale = 0),
        Err(CalibrationConfigError::Widgets(Bad::KnobUnitsOutOfRange { value: 0 }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.knob_units_per_full_scale = 1_000_000),
        Err(CalibrationConfigError::Widgets(Bad::KnobUnitsOutOfRange { .. }))
    ));
    for bad_travel in [0.0, -400.0, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                reject(&|c| c.widgets.knob_full_travel_pt = bad_travel),
                Err(CalibrationConfigError::Widgets(Bad::NonPositive {
                    field: "knob_full_travel_pt",
                    ..
                }))
            ),
            "ход {bad_travel}"
        );
    }
    // 400 pt на 2·1000 единиц — 0,2 pt на единицу: мельче порога различимости, это ошибка конфига.
    assert!(matches!(
        reject(&|c| c.widgets.knob_units_per_full_scale = 1000),
        Err(CalibrationConfigError::Widgets(Bad::KnobScaleTooFine {
            units: 1000,
            ..
        }))
    ));
    // Ровно на пороге (0,5 pt на единицу: 400 pt на 2·400 единиц) конфиг допустим.
    assert!(
        reject(&|c| c.widgets.knob_units_per_full_scale = 400).is_ok(),
        "0,5 pt на единицу — граница допустимого"
    );
    assert!(matches!(
        reject(&|c| c.widgets.knob_tick_every_steps = 0),
        Err(CalibrationConfigError::Widgets(Bad::ZeroTickPeriod))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.knob_pan_hysteresis_pt = 0.0),
        Err(CalibrationConfigError::Widgets(Bad::NonPositive {
            field: "knob_pan_hysteresis_pt",
            ..
        }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.knob_major_every_steps = 0),
        Err(CalibrationConfigError::Widgets(Bad::ZeroMajorPeriod))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.chip_pool_size = 0),
        Err(CalibrationConfigError::Widgets(Bad::ChipPoolOutOfRange { value: 0 }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.chip_pool_size = 256),
        Err(CalibrationConfigError::Widgets(Bad::ChipPoolOutOfRange { value: 256 }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.problem_banner_seconds = 0.0),
        Err(CalibrationConfigError::Widgets(Bad::NonPositive {
            field: "problem_banner_seconds",
            ..
        }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.disabled_row_opacity = 2.0),
        Err(CalibrationConfigError::Widgets(Bad::FractionOutOfRange {
            field: "disabled_row_opacity",
            ..
        }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.row_title_height_pt = c.widgets.row_height_pt),
        Err(CalibrationConfigError::Widgets(Bad::RowTooShort { .. }))
    ));
    // Зона «↻» над строкой не может съесть всю линейку: иначе её у низа строки было бы нечем ловить.
    assert!(matches!(
        reject(&|c| c.widgets.row_reset_reach_above_pt = c.widgets.row_height_pt - c.widgets.row_title_height_pt),
        Err(CalibrationConfigError::Widgets(Bad::ResetReachEatsRuler { .. }))
    ));
}

#[test]
fn wheel_view_numbers_are_validated() {
    use super::CalibrationWidgetsConfigError as Bad;
    let reject = |edit: &dyn Fn(&mut CalibrationConfig)| {
        let mut config = CalibrationConfig::default();
        edit(&mut config);
        config.validate()
    };
    assert!(matches!(
        reject(&|c| c.widgets.wheel_tabs_height_pt = 0.0),
        Err(CalibrationConfigError::Widgets(Bad::NonPositive {
            field: "wheel_tabs_height_pt",
            ..
        }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.wheel_pad_fill_alpha = 1.5),
        Err(CalibrationConfigError::Widgets(Bad::FractionOutOfRange {
            field: "wheel_pad_fill_alpha",
            ..
        }))
    ));
    // Наименьший диаметр больше наибольшего, либо кольцо не уже радиуса наименьшего круга.
    assert!(matches!(
        reject(&|c| c.widgets.wheel_circle_min_diameter_pt = c.widgets.wheel_circle_max_diameter_pt + 1.0),
        Err(CalibrationConfigError::Widgets(Bad::WheelCircleRange { .. }))
    ));
    assert!(matches!(
        reject(&|c| c.widgets.wheel_ring_thickness_pt = c.widgets.wheel_circle_min_diameter_pt),
        Err(CalibrationConfigError::Widgets(Bad::WheelCircleRange { .. }))
    ));
}

#[test]
fn a_bad_widget_number_makes_the_whole_screen_config_invalid() {
    let mut config = ScreenConfig::default();
    config.calibration.widgets.mode_pill_padding_pt = -1.0;
    assert!(matches!(
        config.validate(),
        Err(ScreenConfigError::Calibration(CalibrationConfigError::Widgets(_)))
    ));
}

#[test]
fn defaults_put_the_strip_below_in_portrait_the_scope_under_the_preview_in_landscape_and_the_knobs_on_the_left() {
    let layout = CalibrationConfig::default().layout;
    assert_eq!(layout.portrait.shape, ZoneShape::ScopeStripBelow);
    assert_eq!(layout.landscape.shape, ZoneShape::ScopeUnderPreview);
    assert_eq!(layout.knobs_side, KnobsSide::Left);
}

#[test]
fn the_layout_config_round_trips_through_json_with_named_shapes_and_rejects_unknown_fields() {
    let config = CalibrationConfig::default();
    let json = serde_json::to_string(&config).expect("сериализация");
    assert!(
        json.contains("\"scope_strip_below\""),
        "форма — именованная, а не флаг: {json}"
    );
    assert!(json.contains("\"left\""), "сторона колонки — именованная: {json}");
    let back: CalibrationConfig = serde_json::from_str(&json).expect("чтение");
    assert_eq!(back, config);
    let bad = json.replace("\"side_button_inset_pt\"", "\"unknown_inset\"");
    assert!(serde_json::from_str::<CalibrationConfig>(&bad).is_err());
}
