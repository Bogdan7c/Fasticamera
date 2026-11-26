use super::*;

#[test]
fn defaults_are_valid_and_are_exactly_the_baseline_group() {
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
        (
            SaturationScenarioConfig {
                catalog_wait_ms: 0,
                ..SaturationScenarioConfig::default()
            },
            "catalog_wait_ms",
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

#[test]
fn default_list_is_the_baseline_group_and_groups_expand_in_place() {
    let config = SaturationScenarioConfig::default();
    assert_eq!(config.workloads, baseline_names());
    assert_eq!(config.resolve_workloads().expect("умолчание верно").len(), 14);

    let mixed = SaturationScenarioConfig {
        workloads: vec![
            "still".to_owned(),
            "isp_ablation".to_owned(),
            " ui_ablation ".to_owned(),
        ],
        ..SaturationScenarioConfig::default()
    };
    let names: Vec<&str> = mixed
        .resolve_workloads()
        .expect("группы раскрываются")
        .iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(names.len(), 1 + 14 + 5);
    assert_eq!(names[0], "still", "порядок списка сохраняется");
    assert_eq!(names[1], "viewfinder_awb_never");
    assert_eq!(names[15], "display_calibrate_shapes_only");
    assert_eq!(mixed.validate(), Ok(()));
}

#[test]
fn a_work_listed_again_inside_a_group_and_an_unknown_name_are_config_errors() {
    let duplicate = SaturationScenarioConfig {
        workloads: vec!["viewfinder_minimal".to_owned(), "isp_ablation".to_owned()],
        ..SaturationScenarioConfig::default()
    };
    assert_eq!(
        duplicate.validate(),
        Err(SaturationScenarioConfigError::DuplicateWorkload {
            name: "viewfinder_minimal".to_owned()
        })
    );
    let unknown = SaturationScenarioConfig {
        workloads: vec!["isp_ablatoin".to_owned()],
        ..SaturationScenarioConfig::default()
    };
    assert!(matches!(
        unknown.validate(),
        Err(SaturationScenarioConfigError::UnknownWorkload(_))
    ));
}
