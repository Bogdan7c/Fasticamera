//! Тесты конфига раздела калибровки: умолчания допустимы, плохие числа отвергаются с понятной причиной,
//! конфиг экрана читается из JSON и отвергает неизвестные поля.

use scope_contract::{MAX_SCOPE_IMAGE_SIDE_PX, VectorscopeAxes};

use super::{CalibrationConfig, CalibrationConfigError};
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
                Err(CalibrationConfigError::FractionOutOfRange {
                    field: "landscape.knobs_width_fraction",
                    ..
                })
            ),
            "{bad}"
        );
    }
}

#[test]
fn bottom_row_heights_and_the_scope_limit_are_validated() {
    let mut config = CalibrationConfig::default();
    config.layout.portrait_bottom_row_height_pt = 0.0;
    assert!(matches!(
        config.validate(),
        Err(CalibrationConfigError::InvalidBottomRowHeight { .. })
    ));

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
        reject(&|c| c.widgets.knob_step = 1.5),
        Err(CalibrationConfigError::Widgets(Bad::KnobStepOutOfRange { .. }))
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
