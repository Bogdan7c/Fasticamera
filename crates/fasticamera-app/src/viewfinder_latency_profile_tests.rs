use super::*;

fn named(name: &str) -> LatencyProfileConfig {
    LatencyProfileConfig {
        name: name.to_owned(),
        ..default_latency_profiles().remove(0)
    }
}

#[test]
fn defaults_are_baseline_and_tuned_and_valid() {
    let profiles = default_latency_profiles();

    assert_eq!(validate_latency_profiles(&profiles), Ok(()));
    assert_eq!(profiles[0].name, "baseline");
    assert_eq!(profiles[0].adpf, AdpfMode::Disabled);
    assert_eq!(profiles[0].policy, PresentTimelinePolicy::Preferred);
    assert_eq!(profiles[1].name, "tuned");
    assert_eq!(profiles[1].adpf, AdpfMode::Enabled);
    assert_eq!(profiles[1].late_latch, LateLatchMode::BeforeCameraPoll);
    assert_eq!(
        profiles[1].present_target_setting(),
        PresentTargetSetting {
            policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 },
            late_latch: LateLatchMode::BeforeCameraPoll,
            depth_reduction: DepthReductionMode::SkipOneVsync,
        }
    );
    assert_eq!(profiles[0].viewfinder_pacing, ViewfinderPacing::LatestWins);
    assert_eq!(profiles[0].depth_reduction, DepthReductionMode::Disabled);
    assert_eq!(profiles[1].viewfinder_pacing, ViewfinderPacing::JitterBuffer);
}

#[test]
fn profiles_round_trip_through_the_documented_json_shape() {
    let json = serde_json::to_value(default_latency_profiles()).expect("сериализация");

    assert_eq!(
        json[1],
        serde_json::json!({
            "name": "tuned",
            "policy": {"kind": "nearest_feasible", "margin_us": 1500},
            "late_latch": "before_camera_poll",
            "depth_reduction": "skip_one_vsync",
            "adpf": "enabled",
            "viewfinder_pacing": "jitter_buffer"
        })
    );
    let back: Vec<LatencyProfileConfig> = serde_json::from_value(json).expect("разбор");
    assert_eq!(back, default_latency_profiles());
}

#[test]
fn unknown_profile_key_is_rejected() {
    let json = serde_json::json!({"name": "x", "policy": {"kind": "preferred"}, "late_latch": "disabled",
        "depth_reduction": "disabled", "adpf": "disabled", "viewfinder_pacing": "latest_wins",
        "nearest_margin_us": 5});

    assert!(serde_json::from_value::<LatencyProfileConfig>(json).is_err());
}

#[test]
fn profile_without_new_keys_or_with_unknown_values_is_rejected() {
    let full = serde_json::to_value(default_latency_profiles().remove(0)).expect("сериализация");
    assert!(serde_json::from_value::<LatencyProfileConfig>(full.clone()).is_ok());
    for key in ["depth_reduction", "viewfinder_pacing"] {
        let mut without = full.clone();
        without.as_object_mut().expect("объект").remove(key);
        assert!(
            serde_json::from_value::<LatencyProfileConfig>(without).is_err(),
            "{key} обязателен"
        );
    }
    let mut bad_pacing = full.clone();
    bad_pacing["viewfinder_pacing"] = "triple_buffer".into();
    assert!(serde_json::from_value::<LatencyProfileConfig>(bad_pacing).is_err());
    let mut bad_depth = full;
    bad_depth["depth_reduction"] = "skip_two".into();
    assert!(serde_json::from_value::<LatencyProfileConfig>(bad_depth).is_err());
}

#[test]
fn wrong_count_empty_long_and_duplicate_names_are_rejected() {
    assert_eq!(
        validate_latency_profiles(&[named("a")]),
        Err(LatencyProfilesError::WrongCount { count: 1 })
    );
    assert_eq!(
        validate_latency_profiles(&[named("a"), named("b"), named("c")]),
        Err(LatencyProfilesError::WrongCount { count: 3 })
    );
    assert_eq!(
        validate_latency_profiles(&[named("a"), named("  ")]),
        Err(LatencyProfilesError::EmptyName { index: 1 })
    );
    assert_eq!(
        validate_latency_profiles(&[named("a"), named(&"x".repeat(17))]),
        Err(LatencyProfilesError::NameTooLong { index: 1 })
    );
    assert_eq!(validate_latency_profiles(&[named("a"), named(&"ж".repeat(16))]), Ok(()));
    assert_eq!(
        validate_latency_profiles(&[named("a"), named("a")]),
        Err(LatencyProfilesError::DuplicateName { name: "a".to_owned() })
    );
}
