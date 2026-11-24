use super::*;

#[test]
fn defaults_are_valid_and_cover_the_whole_catalog() {
    let config = SaturationScenarioConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(config.workloads.len(), 14);
    assert_eq!((config.warmup_ms, config.measure_ms), (3_000, 10_000));
    assert_eq!((config.timeline_step_ms, config.cooldown_between_ms), (1_000, 3_000));
    assert_eq!(config.frames_in_flight, 2);
}

#[test]
fn unknown_empty_and_duplicate_workloads_are_config_errors() {
    let unknown = SaturationScenarioConfig {
        workloads: vec!["viewfinder".into(), "nope".into()],
        ..SaturationScenarioConfig::default()
    };
    assert!(matches!(
        unknown.validate(),
        Err(SaturationScenarioConfigError::UnknownWorkload(error)) if error.name == "nope"
    ));
    let empty = SaturationScenarioConfig {
        workloads: Vec::new(),
        ..SaturationScenarioConfig::default()
    };
    assert_eq!(empty.validate(), Err(SaturationScenarioConfigError::NoWorkloads));
    let duplicate = SaturationScenarioConfig {
        workloads: vec!["still".into(), "still".into()],
        ..SaturationScenarioConfig::default()
    };
    assert_eq!(
        duplicate.validate(),
        Err(SaturationScenarioConfigError::DuplicateWorkload { name: "still".into() })
    );
}

#[test]
fn zero_durations_and_bad_slots_are_rejected_with_the_field_name() {
    for (broken, field) in [
        (
            SaturationScenarioConfig {
                measure_ms: 0,
                ..SaturationScenarioConfig::default()
            },
            "measure_ms",
        ),
        (
            SaturationScenarioConfig {
                warmup_ms: 0,
                ..SaturationScenarioConfig::default()
            },
            "warmup_ms",
        ),
        (
            SaturationScenarioConfig {
                timeline_step_ms: 0,
                ..SaturationScenarioConfig::default()
            },
            "timeline_step_ms",
        ),
    ] {
        assert_eq!(broken.validate(), Err(SaturationScenarioConfigError::Zero { field }));
    }
    let no_slots = SaturationScenarioConfig {
        frames_in_flight: 0,
        ..SaturationScenarioConfig::default()
    };
    assert!(matches!(
        no_slots.validate(),
        Err(SaturationScenarioConfigError::FramesInFlight(_))
    ));
}

#[test]
fn cooldown_may_be_zero_and_deadline_grows_with_the_list() {
    let config = SaturationScenarioConfig {
        cooldown_between_ms: 0,
        ..SaturationScenarioConfig::default()
    };
    assert_eq!(config.validate(), Ok(()));
    let one = config.job_deadline(1);
    let fourteen = config.job_deadline(14);
    // 3 + 10 + 0 + 30 с на работу плюс 10 с на закрытие камеры.
    assert_eq!(one, Duration::from_millis(43_000 + 10_000));
    assert_eq!(fourteen, Duration::from_millis(43_000 * 14 + 10_000));
}

#[test]
fn json_section_with_unknown_field_is_rejected_and_partial_section_uses_defaults() {
    let partial: SaturationScenarioConfig =
        serde_json::from_str(r#"{"workloads":["still"],"measure_ms":2000}"#).expect("частичная секция");
    assert_eq!(partial.workloads, vec!["still".to_owned()]);
    assert_eq!(partial.measure_ms, 2_000);
    assert_eq!(partial.warmup_ms, 3_000, "остальное — умолчание");
    assert!(serde_json::from_str::<SaturationScenarioConfig>(r#"{"measure":1}"#).is_err());
}
