//! Тесты конфига экрана: умолчания, разбор JSON, ошибки значений.

use crate::config::{ScreenConfig, ScreenConfigError, ToolsConfig};
use crate::exposure::ExposureConfig;
use crate::focus::FocusConfig;
use crate::level::LevelConfig;
use crate::shutter::TimerConfig;
use crate::state::ScreenState;
use crate::tools::{ToolId, ToolSetError};
use crate::white_button_layout::WhiteButtonLayoutError;
use crate::white_config::WhiteButtonSide;

#[test]
fn default_config_is_valid_and_builds_a_screen() {
    let config = ScreenConfig::default();
    assert_eq!(config.validate(), Ok(()));
    let state = ScreenState::new(&config).expect("экран");
    // Меню v1: порядок iOS 27 (Flash, Aspect, Timer / Exposure, RAW, Grid / Level) без шестерёнки (инвариант 6).
    // Вспышки у камеры пока нет (`FlashAvailability::Unavailable`, решение 138): пункта Flash в меню нет.
    assert_eq!(
        state.menu_tools(),
        [
            ToolId::Aspect,
            ToolId::Timer,
            ToolId::Exposure,
            ToolId::Raw,
            ToolId::Grid,
            ToolId::Level
        ]
    );
    // Таймер выключен — его значка над кадром нет; вспышки нет — значка нет тоже.
    assert_eq!(state.visible_strip_tools(), [ToolId::Raw, ToolId::Exposure]);
}

#[test]
fn config_round_trips_through_json() {
    let config = ScreenConfig::default();
    let json = serde_json::to_string(&config).expect("json");
    let parsed: ScreenConfig = serde_json::from_str(&json).expect("разбор");
    assert_eq!(parsed, config);
}

#[test]
fn partial_json_overrides_only_given_fields_and_keeps_defaults() {
    let parsed: ScreenConfig =
        serde_json::from_str(r#"{"focus": {"indicator_visible_ms": 5000, "lock_confirmation_timeout_ms": 700}}"#)
            .expect("разбор");
    assert_eq!(parsed.focus.indicator_visible_ms, 5000);
    assert_eq!(parsed.focus.lock_confirmation_timeout_ms, 700);
    assert_eq!(parsed.exposure, ExposureConfig::default());
}

#[test]
fn unknown_fields_are_errors_at_every_level() {
    assert!(serde_json::from_str::<ScreenConfig>(r#"{"unknown": 1}"#).is_err());
    assert!(
        serde_json::from_str::<ScreenConfig>(
            r#"{"exposure": {"step_ev": 0.5, "min_ev": -1, "max_ev": 1, "extra": 0}}"#
        )
        .is_err()
    );
    assert!(serde_json::from_str::<ScreenConfig>(r#"{"layout": {"nope": 1}}"#).is_err());
}

#[test]
fn invalid_exposure_values_are_rejected() {
    for exposure in [
        ExposureConfig {
            step_ev: 0.0,
            ..ExposureConfig::default()
        },
        ExposureConfig {
            step_ev: f32::NAN,
            ..ExposureConfig::default()
        },
        ExposureConfig {
            min_ev: 0.5,
            ..ExposureConfig::default()
        },
        ExposureConfig {
            max_ev: -0.5,
            ..ExposureConfig::default()
        },
        ExposureConfig {
            max_ev: f32::INFINITY,
            ..ExposureConfig::default()
        },
    ] {
        let config = ScreenConfig {
            exposure,
            ..ScreenConfig::default()
        };
        assert!(
            matches!(config.validate(), Err(ScreenConfigError::Exposure(_))),
            "{exposure:?}"
        );
        assert!(ScreenState::new(&config).is_err());
    }
}

#[test]
fn invalid_timer_focus_and_level_values_are_rejected() {
    let timer = TimerConfig {
        short_seconds: 5,
        medium_seconds: 5,
        long_seconds: 10,
    };
    assert!(matches!(
        ScreenConfig {
            timer,
            ..ScreenConfig::default()
        }
        .validate(),
        Err(ScreenConfigError::Timer(_))
    ));
    let timer = TimerConfig {
        short_seconds: 0,
        ..TimerConfig::default()
    };
    assert!(
        ScreenConfig {
            timer,
            ..ScreenConfig::default()
        }
        .validate()
        .is_err()
    );
    let focus = FocusConfig {
        indicator_visible_ms: 0,
        ..FocusConfig::default()
    };
    assert!(matches!(
        ScreenConfig {
            focus,
            ..ScreenConfig::default()
        }
        .validate(),
        Err(ScreenConfigError::Focus(_))
    ));
    let focus = FocusConfig {
        lock_confirmation_timeout_ms: 0,
        ..FocusConfig::default()
    };
    assert!(
        ScreenConfig {
            focus,
            ..ScreenConfig::default()
        }
        .validate()
        .is_err()
    );
    let level = LevelConfig {
        flat_threshold_degrees: 95.0,
    };
    assert!(matches!(
        ScreenConfig {
            level,
            ..ScreenConfig::default()
        }
        .validate(),
        Err(ScreenConfigError::Level(_))
    ));
}

#[test]
fn invalid_tool_sets_are_rejected_with_the_reason() {
    let with_tools = |essentials: Vec<ToolId>, controls_menu: Vec<ToolId>| ScreenConfig {
        tools: ToolsConfig {
            essentials,
            controls_menu,
        },
        ..ScreenConfig::default()
    };
    let menu = ToolsConfig::default().controls_menu;
    assert_eq!(
        with_tools(vec![ToolId::Live], menu.clone()).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::NotImplemented(ToolId::Live)))
    );
    assert_eq!(
        with_tools(vec![ToolId::Flash, ToolId::Flash], menu.clone()).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::Duplicate(ToolId::Flash)))
    );
    assert_eq!(
        with_tools(vec![ToolId::Aspect], menu).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::NotAStripIcon(ToolId::Aspect)))
    );
    assert_eq!(
        with_tools(vec![ToolId::Flash], Vec::new()).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::EmptyMenu))
    );
    assert_eq!(
        with_tools(vec![ToolId::Flash], vec![ToolId::Night]).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::NotImplemented(ToolId::Night)))
    );
}

#[test]
fn tool_ids_parse_from_snake_case_names() {
    let parsed: ToolsConfig =
        serde_json::from_str(r#"{"essentials": ["flash", "exposure"], "controls_menu": ["aspect"]}"#).expect("разбор");
    assert_eq!(parsed.essentials, [ToolId::Flash, ToolId::Exposure]);
    assert!(parsed.tool_set().is_ok());
}

#[test]
fn every_capture_limit_must_be_positive() {
    use crate::shutter::CaptureConfigError;
    assert!(ScreenConfig::default().validate().is_ok());
    type Break = fn(&mut ScreenConfig);
    let cases: [(Break, CaptureConfigError); 3] = [
        (
            |c| c.capture.capture_timeout_ms = 0,
            CaptureConfigError::ZeroCaptureTimeout,
        ),
        (
            |c| c.capture.saving_timeout_ms = 0,
            CaptureConfigError::ZeroSavingTimeout,
        ),
        (
            |c| c.capture.max_saving_shots = 0,
            CaptureConfigError::ZeroSavingCapacity,
        ),
    ];
    for (break_it, expected) in cases {
        let mut config = ScreenConfig::default();
        break_it(&mut config);
        assert_eq!(
            config.validate(),
            Err(ScreenConfigError::Capture(expected)),
            "{expected:?}"
        );
    }
}

#[test]
fn capture_defaults_follow_decision_140_and_primary_format_comes_from_json() {
    let defaults = ScreenConfig::default().capture;
    assert_eq!(defaults.max_saving_shots, 10, "решение 140");
    assert_eq!(
        defaults.saving_timeout_ms, 45_000,
        "срок службы: проявление 30 с + кодер 5 с + запас 10 с"
    );
    assert!(defaults.capture_timeout_ms < defaults.saving_timeout_ms);
    assert_eq!(defaults.primary_format, still_contract::plan::PrimaryFormat::Heif);
    let parsed: ScreenConfig = serde_json::from_str(r#"{"capture": {"primary_format": "jpeg"}}"#).expect("разбор");
    assert_eq!(parsed.capture.primary_format, still_contract::plan::PrimaryFormat::Jpeg);
    assert_eq!(parsed.capture.max_saving_shots, 10, "остальное — по умолчанию");
    assert!(serde_json::from_str::<ScreenConfig>(r#"{"capture": {"result_timeout_ms": 1}}"#).is_err());
}

// ---- кнопка белого у затвора (WB35) ----

#[test]
fn white_is_neither_a_strip_icon_nor_a_menu_item_and_the_default_strip_is_unchanged() {
    let with_tools = |essentials: Vec<ToolId>, controls_menu: Vec<ToolId>| ScreenConfig {
        tools: ToolsConfig {
            essentials,
            controls_menu,
        },
        ..ScreenConfig::default()
    };
    assert_eq!(
        ToolsConfig::default().essentials,
        [ToolId::Flash, ToolId::Raw, ToolId::Timer, ToolId::Exposure]
    );
    assert_eq!(
        with_tools(vec![ToolId::White], vec![ToolId::Aspect]).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::NotAStripIcon(ToolId::White)))
    );
    assert_eq!(
        with_tools(vec![ToolId::Flash], vec![ToolId::Aspect, ToolId::White]).validate(),
        Err(ScreenConfigError::Tools(ToolSetError::NotAMenuItem(ToolId::White)))
    );
}

#[test]
fn button_side_parses_both_values_and_defaults_to_the_right() {
    assert_eq!(ScreenConfig::default().white.button_side, WhiteButtonSide::Right);
    for (word, side) in [("right", WhiteButtonSide::Right), ("left", WhiteButtonSide::Left)] {
        let parsed: ScreenConfig =
            serde_json::from_str(&format!(r#"{{"white": {{"button_side": "{word}"}}}}"#)).expect("разбор");
        assert_eq!(parsed.white.button_side, side, "{word}");
        assert_eq!(parsed.validate(), Ok(()));
    }
}

#[test]
fn an_unknown_button_side_is_rejected_and_the_error_names_the_field() {
    for bad in [r#""up""#, r#""Right""#, r#""""#] {
        let error = serde_json::from_str::<ScreenConfig>(&format!(r#"{{"white": {{"button_side": {bad}}}}}"#))
            .expect_err(bad)
            .to_string();
        assert!(
            error.contains("camera_screen.screen.white.button_side"),
            "{bad}: {error}"
        );
    }
    // Не строка вовсе — тоже отказ, а не молчаливое умолчание.
    assert!(serde_json::from_str::<ScreenConfig>(r#"{"white": {"button_side": 3}}"#).is_err());
}

#[test]
fn the_button_offset_must_keep_the_button_clear_of_the_shutter_and_the_menu_button() {
    let mut config = ScreenConfig::default();
    let bottom = config.widgets.bottom.clone();
    let shutter_diameter_pt = 80.1;
    assert_eq!(
        config
            .widgets
            .white_button
            .check_clearance(&bottom, shutter_diameter_pt),
        Ok(())
    );
    // Слишком близко к затвору: наезд на затвор (ближайший сосед с обеих сторон).
    config.widgets.white_button.offset_from_shutter_pt = 50.0;
    assert!(matches!(
        config
            .widgets
            .white_button
            .check_clearance(&bottom, shutter_diameter_pt),
        Err(WhiteButtonLayoutError::TooCloseTo {
            neighbour: "затвор",
            ..
        })
    ));
    // Слишком далеко: справа наезжает на кнопку меню (слева она ушла бы за край экрана, но справа проверка раньше).
    config.widgets.white_button.offset_from_shutter_pt = 140.0;
    assert!(matches!(
        config
            .widgets
            .white_button
            .check_clearance(&bottom, shutter_diameter_pt),
        Err(WhiteButtonLayoutError::TooCloseTo {
            neighbour: "кнопка меню",
            side: WhiteButtonSide::Right,
            ..
        })
    ));
    // Левая сторона сама по себе: огромное смещение уносит кнопку за левый край.
    config.widgets.white_button.offset_from_shutter_pt = 250.0;
    assert!(
        config
            .widgets
            .white_button
            .check_clearance(&bottom, shutter_diameter_pt)
            .is_err()
    );
    // Числа самой раскладки.
    config.widgets.white_button.offset_from_shutter_pt = -1.0;
    assert!(matches!(config.validate(), Err(ScreenConfigError::Widgets(_))));
}

#[test]
fn a_screen_with_a_button_that_overlaps_the_shutter_is_not_built() {
    use crate::screen::{CameraScreen, ScreenBuildError};
    use ui_core::time::MotionPreference;
    use ui_kit::tokens::UiKitTokens;
    let mut config = ScreenConfig::default();
    config.widgets.white_button.offset_from_shutter_pt = 50.0;
    let built = CameraScreen::new(config, UiKitTokens::default(), MotionPreference::Full);
    assert!(
        matches!(built, Err(ScreenBuildError::WhiteButton(_))),
        "{:?}",
        built.err()
    );
}
