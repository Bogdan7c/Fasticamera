//! Конфиг панели белого: умолчания, JSON, проверка со всеми ошибками и сверка с пределами движка.

use crate::config::{ScreenConfig, ScreenConfigError};
use crate::white_config::{WhiteConfigError, WhiteLight, WhiteLightIcon, WhiteScreenConfig};
use crate::white_facts::WhiteRange;
use ui_draw::icons::IconId;

fn light(label: &str, kelvin: f64, duv: f64) -> WhiteLight {
    WhiteLight {
        label: label.to_owned(),
        icon: WhiteLightIcon::Sun,
        kelvin,
        duv,
    }
}

fn config_with(lights: Vec<WhiteLight>) -> WhiteScreenConfig {
    WhiteScreenConfig {
        lights,
        ..WhiteScreenConfig::default()
    }
}

#[test]
fn default_config_is_valid_and_lists_the_five_lights_from_the_plan() {
    let config = WhiteScreenConfig::default();
    assert_eq!(config.all_errors(), []);
    let numbers: Vec<_> = config
        .lights
        .iter()
        .map(|light| (light.label.as_str(), light.kelvin, light.duv))
        .collect();
    assert_eq!(
        numbers,
        [
            ("Sun", 5_600.0, 0.003),
            ("Cloud", 6_500.0, 0.003),
            ("Shade", 7_500.0, 0.003),
            ("Lamp", 3_200.0, 0.0),
            ("Home", 2_700.0, 0.0),
        ]
    );
    // «Телефон» + пять кнопок света помещаются без прокрутки.
    assert_eq!(config.max_lights_without_scroll(), 5);
}

#[test]
fn default_lights_are_inside_the_default_engine_range() {
    assert_eq!(
        WhiteScreenConfig::default().check_lights_in_range(&WhiteRange::STARTING),
        Ok(())
    );
}

#[test]
fn icons_are_read_from_snake_case_json_and_map_to_the_atlas() {
    let parsed: WhiteLight =
        serde_json::from_str(r#"{"label": "Phone", "icon": "device_mobile", "kelvin": 5000.0, "duv": 0.0}"#)
            .expect("разбор");
    assert_eq!(parsed.icon, WhiteLightIcon::DeviceMobile);
    assert_eq!(parsed.icon.icon_id(), IconId::DeviceMobile);
    assert_eq!(WhiteLightIcon::Tree.icon_id(), IconId::Tree);
    assert_eq!(WhiteLightIcon::Lightbulb.icon_id(), IconId::Lightbulb);
}

#[test]
fn unknown_json_field_and_unknown_icon_are_errors() {
    assert!(serde_json::from_str::<WhiteScreenConfig>(r#"{"nonsense": 1}"#).is_err());
    assert!(
        serde_json::from_str::<WhiteLight>(r#"{"label": "X", "icon": "moon", "kelvin": 1.0, "duv": 0.0}"#).is_err()
    );
}

#[test]
fn white_config_round_trips_through_json() {
    let config = WhiteScreenConfig::default();
    let parsed: WhiteScreenConfig =
        serde_json::from_str(&serde_json::to_string(&config).expect("json")).expect("разбор");
    assert_eq!(parsed, config);
}

#[test]
fn validation_reports_every_problem_not_just_the_first() {
    let config = WhiteScreenConfig {
        lights: vec![
            light("", 5_000.0, 0.0),
            light("Dup", 5_000.0, 0.0),
            light("Dup", 5_000.0, 0.0),
            light("Broken", f64::NAN, 0.0),
            light("Negative", -1.0, 0.0),
            light("Tinted", 5_000.0, f64::INFINITY),
        ],
        mired_step: 0.0,
        ..WhiteScreenConfig::default()
    };
    let errors = config.all_errors();
    assert!(
        errors.contains(&WhiteConfigError::EmptyLabel { index: 0 }),
        "{errors:?}"
    );
    assert!(
        errors.contains(&WhiteConfigError::DuplicateLabel {
            label: "Dup".to_owned()
        }),
        "{errors:?}"
    );
    assert!(
        errors.contains(&WhiteConfigError::TooManyLights {
            count: 6,
            max_without_scroll: 5
        }),
        "{errors:?}"
    );
    for broken in ["Broken", "Negative", "Tinted"] {
        assert!(
            errors
                .iter()
                .any(|error| matches!(error, WhiteConfigError::InvalidLight { label, .. } if label == broken)),
            "{broken}: {errors:?}"
        );
    }
    assert!(
        errors.contains(&WhiteConfigError::NotPositive {
            field: "mired_step",
            value: 0.0
        }),
        "{errors:?}"
    );
    // Первая из них уходит в общую проверку экрана.
    assert!(config.validate().is_err());
}

#[test]
fn an_empty_list_of_lights_is_an_error() {
    assert_eq!(config_with(Vec::new()).all_errors(), [WhiteConfigError::NoLights]);
}

#[test]
fn bad_numbers_of_scales_and_display_are_errors() {
    let config = WhiteScreenConfig {
        display_step_kelvin: f64::NAN,
        tint_units_per_duv: -1.0,
        mired_major_every: 0,
        max_buttons_without_scroll: 0,
        ..WhiteScreenConfig::default()
    };
    let errors = config.all_errors();
    assert!(errors.iter().any(|e| matches!(
        e,
        WhiteConfigError::NotPositive {
            field: "display_step_kelvin",
            ..
        }
    )));
    assert!(errors.iter().any(|e| matches!(
        e,
        WhiteConfigError::NotPositive {
            field: "tint_units_per_duv",
            ..
        }
    )));
    assert!(errors.contains(&WhiteConfigError::ZeroPeriod {
        field: "mired_major_every",
        value: 0
    }));
    assert!(errors.contains(&WhiteConfigError::ZeroPeriod {
        field: "max_buttons_without_scroll",
        value: 0
    }));
}

#[test]
fn screen_config_rejects_a_bad_white_section_and_screen_cannot_be_built_from_it() {
    let config = ScreenConfig {
        white: config_with(Vec::new()),
        ..ScreenConfig::default()
    };
    assert_eq!(
        config.validate(),
        Err(ScreenConfigError::White(WhiteConfigError::NoLights))
    );
    assert!(crate::state::ScreenState::new(&config).is_err());
}

#[test]
fn range_check_names_the_first_light_outside_the_engine_range() {
    let config = config_with(vec![
        light("Fine", 5_000.0, 0.0),
        light("TooCold", 1_500.0, 0.0),
        light("TooGreen", 5_000.0, 0.2),
    ]);
    assert_eq!(
        config.check_lights_in_range(&WhiteRange::STARTING),
        Err(WhiteConfigError::LightOutOfRange {
            label: "TooCold".to_owned()
        })
    );
    // Другие пределы — другой ответ: с широкой шкалой «TooCold» проходит, и называется следующая.
    let wide = WhiteRange {
        min_kelvin: 1_000.0,
        ..WhiteRange::STARTING
    };
    assert_eq!(
        config.check_lights_in_range(&wide),
        Err(WhiteConfigError::LightOutOfRange {
            label: "TooGreen".to_owned()
        })
    );
}

#[test]
fn range_includes_its_borders() {
    let config = config_with(vec![light("Low", 2_000.0, -0.05), light("High", 12_000.0, 0.05)]);
    assert_eq!(config.check_lights_in_range(&WhiteRange::STARTING), Ok(()));
}

#[test]
fn a_light_label_outside_the_font_atlas_is_rejected_at_startup_by_name() {
    let config = config_with(vec![light("Sun\u{2603}", 5_600.0, 0.0)]);
    assert!(config.all_errors().contains(&WhiteConfigError::LabelNotInAtlas {
        label: "Sun\u{2603}".to_owned(),
        character: '\u{2603}',
    }));
    let screen = ScreenConfig {
        white: config,
        ..ScreenConfig::default()
    };
    assert!(matches!(
        screen.validate(),
        Err(ScreenConfigError::White(WhiteConfigError::LabelNotInAtlas { .. }))
    ));
}
