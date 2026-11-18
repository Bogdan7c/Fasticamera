use super::*;

#[test]
fn defaults_match_the_probe_and_plan_the_stream() {
    let config = CompanionStreamConfig::default();
    assert_eq!(config.validate(), Ok(()));
    assert_eq!(
        config.plan(),
        Some(CompanionStreamSpec {
            size: PixelSize::new(640, 480),
            max_images: 4
        })
    );
}

#[test]
fn disabled_config_plans_nothing() {
    let config = CompanionStreamConfig {
        enabled: false,
        ..CompanionStreamConfig::default()
    };
    assert_eq!(config.plan(), None);
}

#[test]
fn out_of_range_values_are_rejected_with_the_value() {
    let with = |change: fn(&mut CompanionStreamConfig)| {
        let mut config = CompanionStreamConfig::default();
        change(&mut config);
        config.validate()
    };
    assert_eq!(
        with(|c| c.width = 0),
        Err(CompanionConfigError::SizeOutOfRange {
            width: 0,
            height: 480,
            max: 1920
        })
    );
    assert_eq!(
        with(|c| c.height = 5000),
        Err(CompanionConfigError::SizeOutOfRange {
            width: 640,
            height: 5000,
            max: 1920
        })
    );
    assert_eq!(
        with(|c| c.max_images = 0),
        Err(CompanionConfigError::MaxImagesOutOfRange { actual: 0, max: 8 })
    );
}

#[test]
fn only_disabled_or_unavailable_companion_means_dead_hal_three_a() {
    assert!(!CompanionStreamStatus::NotConfigured.hal_three_a_is_dead());
    assert!(
        !CompanionStreamStatus::Active {
            width: 640,
            height: 480
        }
        .hal_three_a_is_dead()
    );
    assert!(CompanionStreamStatus::DisabledByConfig.hal_three_a_is_dead());
    assert!(
        CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::RejectedByHal
        }
        .hal_three_a_is_dead()
    );
}

#[test]
fn status_serializes_with_a_readable_reason() {
    let status = CompanionStreamStatus::Unavailable {
        reason: CompanionUnavailableReason::ReaderCreationFailed { status: -10000 },
    };
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["state"], "unavailable");
    assert_eq!(json["reason"]["reason"], "reader_creation_failed");
    assert_eq!(json["reason"]["status"], -10000);
}
