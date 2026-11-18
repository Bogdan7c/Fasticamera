//! Конфиги AE: значения по умолчанию верны, `validate` ловит ошибки, неизвестные поля отвергаются.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use super::*;

#[test]
fn defaults_are_valid_and_round_trip_through_json() {
    MeteringConfig::default().validate().unwrap();
    ExposureControllerConfig::default().validate().unwrap();
    let json = serde_json::to_string(&ExposureControllerConfig::default()).unwrap();
    let parsed: ExposureControllerConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, ExposureControllerConfig::default());
}

#[test]
fn unknown_fields_are_rejected() {
    assert!(serde_json::from_str::<MeteringConfig>(r#"{"nonsense": 1}"#).is_err());
    assert!(serde_json::from_str::<ExposureControllerConfig>(r#"{"nonsense": 1}"#).is_err());
    // Частичный конфиг дополняется значениями по умолчанию.
    let partial: MeteringConfig = serde_json::from_str(r#"{"max_step_up_ev": 2.0}"#).unwrap();
    assert_eq!(partial.max_step_up_ev, 2.0);
    assert_eq!(partial.max_step_down_ev, MeteringConfig::default().max_step_down_ev);
}

#[test]
fn metering_config_rejects_negative_nan_and_inverted_thresholds() {
    let cases: [(&str, MeteringConfig); 9] = [
        (
            "severe below limit",
            MeteringConfig {
                severe_clipped_fraction: 0.3,
                ..MeteringConfig::default()
            },
        ),
        (
            "sigma",
            MeteringConfig {
                matrix_center_sigma: -0.1,
                ..MeteringConfig::default()
            },
        ),
        (
            "background",
            MeteringConfig {
                spot_background_weight: f32::NAN,
                ..MeteringConfig::default()
            },
        ),
        (
            "floor",
            MeteringConfig {
                dark_floor: 0.0,
                ..MeteringConfig::default()
            },
        ),
        (
            "tile darker than floor",
            MeteringConfig {
                dark_tile_luminance: 0.0005,
                ..MeteringConfig::default()
            },
        ),
        (
            "clip limit",
            MeteringConfig {
                clipped_fraction_limit: 1.5,
                ..MeteringConfig::default()
            },
        ),
        (
            "protect start above limit",
            MeteringConfig {
                highlight_protect_start: 0.4,
                ..MeteringConfig::default()
            },
        ),
        (
            "key offset",
            MeteringConfig {
                key_offset_ev: f32::INFINITY,
                ..MeteringConfig::default()
            },
        ),
        (
            "step",
            MeteringConfig {
                max_step_down_ev: -2.0,
                ..MeteringConfig::default()
            },
        ),
    ];
    for (name, config) in cases {
        assert!(config.validate().is_err(), "{name}: конфиг должен быть отвергнут");
    }
}

#[test]
fn controller_config_rejects_negative_nan_and_inverted_dead_band() {
    let cases: [(&str, ExposureControllerConfig); 7] = [
        (
            "unsettle below tolerance",
            ExposureControllerConfig {
                unsettle_tolerance_ev: 0.05,
                ..Default::default()
            },
        ),
        (
            "tau",
            ExposureControllerConfig {
                photo_time_constant_seconds: 0.0,
                ..Default::default()
            },
        ),
        (
            "video tau",
            ExposureControllerConfig {
                video_time_constant_seconds: f32::NAN,
                ..Default::default()
            },
        ),
        (
            "inverted dead band",
            ExposureControllerConfig {
                video_dead_band_start_ev: 0.05,
                video_dead_band_stop_ev: 0.25,
                ..Default::default()
            },
        ),
        (
            "tolerance",
            ExposureControllerConfig {
                settled_tolerance_ev: -0.1,
                ..Default::default()
            },
        ),
        (
            "lock timeout",
            ExposureControllerConfig {
                lock_settle_timeout_seconds: 0.0,
                ..Default::default()
            },
        ),
        (
            "residual",
            ExposureControllerConfig {
                max_residual_ev: f32::NAN,
                ..Default::default()
            },
        ),
    ];
    for (name, config) in cases {
        assert!(config.validate().is_err(), "{name}: конфиг должен быть отвергнут");
    }
}
