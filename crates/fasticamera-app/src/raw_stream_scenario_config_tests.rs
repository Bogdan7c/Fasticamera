use super::*;

#[test]
fn defaults_are_valid_and_pick_the_documented_measure_lengths() {
    let config = RawStreamScenarioConfig::default();
    config.validate().expect("умолчания согласованы");
    assert_eq!(config.measure_ms(RawStreamRunKind::Standard), 60_000);
    assert_eq!(config.measure_ms(RawStreamRunKind::Endurance), 300_000);
    assert_eq!(config.measure_ms(RawStreamRunKind::CpuProbe), 5_000);
}

#[test]
fn resolve_gives_the_main_camera_by_role_with_the_configured_mode() {
    let resolved = RawStreamScenarioConfig::default().resolve().expect("режим");
    assert_eq!(
        resolved.target,
        CameraTarget::Role(camera_contract::ids::LensRole::Wide)
    );
    assert_eq!(resolved.mode.frame_rate.frames_per_second(), 60);
}

#[test]
fn zero_durations_and_bad_fractions_are_rejected_by_name() {
    type Mutate = fn(&mut RawStreamScenarioConfig);
    let zero_cases: [(&str, Mutate); 4] = [
        ("raw_stream.measure_ms", |c| c.measure_ms = 0),
        ("raw_stream.progress_interval_ms", |c| c.progress_interval_ms = 0),
        ("raw_stream.latency_samples_limit", |c| c.latency_samples_limit = 0),
        ("raw_stream.render_live_timeout_ms", |c| c.render_live_timeout_ms = 0),
    ];
    for (field, mutate) in zero_cases {
        let mut config = RawStreamScenarioConfig::default();
        mutate(&mut config);
        assert_eq!(config.validate(), Err(RawStreamScenarioConfigError::Zero { field }));
    }
    let fraction_cases: [(&str, Mutate); 4] = [
        ("raw_stream.min_fraction_of_target", |c| c.min_fraction_of_target = 0.0),
        ("raw_stream.min_presented_fraction", |c| c.min_presented_fraction = 1.5),
        ("raw_stream.min_paired_fraction", |c| c.min_paired_fraction = f64::NAN),
        ("raw_stream.min_paired_fraction", |c| c.min_paired_fraction = -0.1),
    ];
    for (field, mutate) in fraction_cases {
        let mut config = RawStreamScenarioConfig::default();
        mutate(&mut config);
        let error = config.validate().expect_err(field);
        assert!(
            matches!(error, RawStreamScenarioConfigError::FractionOutOfRange { field: reported, .. } if reported == field),
            "{error}"
        );
    }
}

#[test]
fn zero_frame_rate_is_rejected_as_an_invalid_rate() {
    let config = RawStreamScenarioConfig {
        frame_rate: 0,
        ..RawStreamScenarioConfig::default()
    };
    assert!(matches!(
        config.validate(),
        Err(RawStreamScenarioConfigError::InvalidFrameRate(_))
    ));
}

#[test]
fn typo_in_a_field_name_is_rejected_when_parsing() {
    let error = serde_json::from_str::<RawStreamScenarioConfig>(r#"{"measure_sec":5}"#).expect_err("опечатка");
    assert!(error.to_string().contains("measure_sec"), "{error}");
}
