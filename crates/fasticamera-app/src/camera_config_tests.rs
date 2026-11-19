//! Секция `camera`: умолчания, частичное переопределение, опечатки и неверные значения.

use super::*;
use crate::runtime_config::{ConfigError, RuntimeConfig};

fn config_with_camera(camera_section: &str) -> Result<RuntimeConfig, ConfigError> {
    RuntimeConfig::from_json(&format!(
        r#"{{"build_type":"debug","log_filter":"info","camera":{camera_section}}}"#
    ))
}

#[test]
fn absent_section_gives_the_documented_defaults_of_every_owner() {
    let config = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(*config.camera(), CameraConfig::default());
    assert_eq!(config.camera().engine, CameraEngineConfig::default());
    assert_eq!(config.camera().android, AndroidCameraConfig::default());
    assert_eq!(config.camera().scenarios.stream_steps.len(), 6);
    config.camera().validate().expect("умолчания согласованы");
}

#[test]
fn partial_section_overrides_only_named_fields() {
    let config = config_with_camera(
        r#"{"engine":{"recovery":{"max_consecutive_attempts":7}},"android":{"frame_number_window":6},"raw_stream":{"pairing_timeout_ms":80},"scenarios":{"measure_ms":2000}}"#,
    )
    .expect("частичная секция");
    let camera = config.camera();
    assert_eq!(camera.engine.recovery.max_consecutive_attempts, 7);
    assert_eq!(
        camera.engine.recovery.retry_backoff_ms,
        CameraEngineConfig::default().recovery.retry_backoff_ms,
        "несказанное берёт значение по умолчанию"
    );
    assert_eq!(camera.android.frame_number_window, 6);
    assert_eq!(camera.raw_stream.pairing_timeout_ms, 80);
    assert_eq!(
        camera.raw_stream.max_pending_frames,
        RawFrameStreamConfig::default().max_pending_frames,
        "несказанное в raw_stream берёт значение по умолчанию"
    );
    assert_eq!(camera.scenarios.measure_ms, 2000);
    assert_eq!(
        camera.scenarios.catalog_timeout_ms,
        CameraScenarioConfig::default().catalog_timeout_ms
    );
}

#[test]
fn typos_at_every_level_are_rejected() {
    for typo in [
        r#"{"engin":{}}"#,
        r#"{"engine":{"recovry":{}}}"#,
        r#"{"android":{"frame_number_windo":4}}"#,
        r#"{"raw_stream":{"pairing_timeout":50}}"#,
        r#"{"scenarios":{"measure_sec":5}}"#,
        r#"{"scenarios":{"evict":{"rol":"wide"}}}"#,
    ] {
        let error = config_with_camera(typo).expect_err(typo);
        assert!(matches!(error, ConfigError::Malformed(_)), "{typo}: {error}");
    }
}

#[test]
fn bad_values_are_reported_by_the_part_that_owns_them() {
    let engine = config_with_camera(r#"{"engine":{"recovery":{"max_consecutive_attempts":0}}}"#).expect_err("engine");
    assert!(
        matches!(engine, ConfigError::Camera(CameraConfigError::Engine(_))),
        "{engine}"
    );
    let android = config_with_camera(r#"{"android":{"frame_number_window":0}}"#).expect_err("android");
    assert!(
        matches!(android, ConfigError::Camera(CameraConfigError::Android(_))),
        "{android}"
    );
    let raw_stream = config_with_camera(r#"{"raw_stream":{"max_pending_frames":0}}"#).expect_err("raw_stream");
    assert!(
        matches!(raw_stream, ConfigError::Camera(CameraConfigError::RawStream(_))),
        "{raw_stream}"
    );
    assert!(
        raw_stream.to_string().contains("camera.raw_stream.max_pending_frames"),
        "ошибка называет поле: {raw_stream}"
    );
    let scenarios = config_with_camera(r#"{"scenarios":{"stream_steps":[]}}"#).expect_err("scenarios");
    assert!(
        matches!(
            scenarios,
            ConfigError::Camera(CameraConfigError::Scenarios(CameraScenarioConfigError::NoStreamSteps))
        ),
        "{scenarios}"
    );
    assert!(scenarios.to_string().contains("camera.scenarios"), "{scenarios}");
}

#[test]
fn unknown_enum_value_in_a_step_is_rejected_at_parse_time() {
    let error = config_with_camera(
        r#"{"scenarios":{"stream_steps":[{"camera_id":"2","raw_format":"raw12","readout":"four_by_three","frame_rate":60,"expected_lever":"any","min_measured_fps":55.0}]}}"#,
    )
    .expect_err("raw12");
    assert!(matches!(error, ConfigError::Malformed(_)), "{error}");
}

#[test]
fn request_origin_table_smaller_than_a_bracket_plus_margin_is_rejected_and_the_boundary_passes() {
    let bracket_limit = CameraEngineConfig::default().three_a.max_bracket_frames;
    let too_small = bracket_limit + REQUEST_ORIGIN_MARGIN - 1;
    let error = config_with_camera(&format!(r#"{{"android":{{"request_origin_capacity":{too_small}}}}}"#))
        .expect_err("таблица мала для серии");
    assert!(error.to_string().contains("request_origin_capacity"), "{error}");
    let exact = bracket_limit + REQUEST_ORIGIN_MARGIN;
    config_with_camera(&format!(r#"{{"android":{{"request_origin_capacity":{exact}}}}}"#)).expect("граница проходит");
    // Растёт предел серии — таблицу по умолчанию (64) он тоже должен вытеснить из допустимого.
    let error = config_with_camera(r#"{"engine":{"three_a":{"max_bracket_frames":60}}}"#)
        .expect_err("серия 60 + запас не влезает в 64");
    assert!(error.to_string().contains("max_bracket_frames"), "{error}");
}

#[test]
fn three_a_sections_of_every_owner_have_valid_defaults_and_arrive_through_runtime_json() {
    use camera_contract::three_a::MainsFrequency;
    let defaults = RuntimeConfig::from_json(r#"{"build_type":"debug","log_filter":"info"}"#).expect("конфиг");
    assert_eq!(defaults.camera().engine.three_a.max_bracket_frames, 4);
    assert_eq!(
        defaults.camera().android.mains_frequency,
        MainsFrequency::Hz50,
        "решение 67"
    );
    assert!(defaults.camera().android.three_a_companion_stream.enabled);
    assert_eq!(defaults.render().viewfinder.white_balance.statistics_every_n_frames, 4);
    assert_eq!(
        defaults.render().viewfinder.white_balance.max_hidden_frames,
        20,
        "решение 76"
    );

    let configured = RuntimeConfig::from_json(
        r#"{"build_type":"debug","log_filter":"info",
            "camera":{"engine":{"three_a":{"focus_settle_timeout_ms":2000}},
                      "android":{"mains_frequency":"60hz","three_a_companion_stream":{"width":320}}},
            "render":{"viewfinder":{"white_balance":{"statistics_every_n_frames":2,"max_hidden_frames":6}}}}"#,
    )
    .expect("секции 3A всех владельцев");
    assert_eq!(configured.camera().engine.three_a.focus_settle_timeout_ms, 2_000);
    assert_eq!(configured.camera().android.mains_frequency, MainsFrequency::Hz60);
    assert_eq!(configured.camera().android.three_a_companion_stream.width, 320);
    assert_eq!(
        configured.render().viewfinder.white_balance.statistics_every_n_frames,
        2
    );
    assert_eq!(configured.render().viewfinder.white_balance.max_hidden_frames, 6);
}

#[test]
fn three_a_typos_and_invalid_values_are_rejected_naming_the_owner() {
    let reject = |json: &str| {
        RuntimeConfig::from_json(json)
            .expect_err("значение должно отвергаться")
            .to_string()
    };
    let base = |camera: &str, render: &str| {
        format!(r#"{{"build_type":"debug","log_filter":"info","camera":{camera},"render":{render}}}"#)
    };
    assert!(reject(&base(r#"{"engine":{"three_a":{"focus_setle_timeout_ms":1}}}"#, "{}")).contains("focus_setle"));
    assert!(reject(&base(r#"{"android":{"mains_frequency":"55hz"}}"#, "{}")).contains("55hz"));
    assert!(
        reject(&base(
            "{}",
            r#"{"viewfinder":{"white_balance":{"statistics_every_n_frames":0}}}"#
        ))
        .contains("statistics_every_n_frames")
    );
    assert!(reject(&base(r#"{"android":{"three_a_companion_stream":{"width":0}}}"#, "{}")).contains("camera.android"));
    assert!(
        reject(&base(
            r#"{"engine":{"three_a":{"metering_region_size_fraction":0}}}"#,
            "{}"
        ))
        .contains("metering_region_size_fraction")
    );
}

#[test]
fn exposure_controller_defaults_to_ours_and_accepts_both_names_but_not_typos() {
    use crate::auto_exposure_link::ExposureControllerKind;

    assert_eq!(CameraConfig::default().exposure_controller, ExposureControllerKind::Own);
    let vendor = config_with_camera(r#"{"exposure_controller":"vendor"}"#).expect("vendor");
    assert_eq!(vendor.camera().exposure_controller, ExposureControllerKind::Vendor);
    let own = config_with_camera(r#"{"exposure_controller":"own"}"#).expect("own");
    assert_eq!(own.camera().exposure_controller, ExposureControllerKind::Own);
    for typo in [r#"{"exposure_controler":"own"}"#, r#"{"exposure_controller":"auto"}"#] {
        assert!(
            matches!(config_with_camera(typo), Err(ConfigError::Malformed(_))),
            "{typo}"
        );
    }
}

#[test]
fn own_ae_settings_of_the_engine_are_read_from_the_app_config_and_defaulted() {
    let config = config_with_camera(
        r#"{"engine":{"three_a":{"exposure":{"initial_exposure_seconds_iso":2.5,"flicker":"always_protect"}}}}"#,
    )
    .expect("секция нашего AE");
    let exposure = config.camera().engine.three_a.exposure;
    assert_eq!(exposure.initial_exposure_seconds_iso, 2.5);
    assert_eq!(exposure.flicker, camera_core::three_a_own::FlickerPolicy::AlwaysProtect);
    assert_eq!(
        exposure.replan_min_change_ev,
        camera_core::three_a_own::OwnExposureConfig::default().replan_min_change_ev,
        "несказанное берёт значение по умолчанию"
    );
    let bad = config_with_camera(r#"{"engine":{"three_a":{"exposure":{"initial_exposure_seconds_iso":0.0}}}}"#)
        .expect_err("нулевое стартовое значение отвергается при разборе");
    assert!(bad.to_string().contains("initial_exposure_seconds_iso"), "{bad}");
    assert!(config_with_camera(r#"{"engine":{"three_a":{"exposure":{"initial_exposure":1.0}}}}"#).is_err());
}
