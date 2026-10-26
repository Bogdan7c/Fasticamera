use super::*;

#[test]
fn defaults_are_valid_and_partial_json_keeps_other_defaults() {
    assert_eq!(CameraEngineConfig::default().validate(), Ok(()));
    let parsed: CameraEngineConfig =
        serde_json::from_str(r#"{"runtime":{"drop_shutdown_timeout_ms":500}}"#).expect("частичный конфиг");
    assert_eq!(parsed.runtime.drop_shutdown_timeout_ms, 500);
    assert_eq!(parsed.recovery, RecoveryConfig::default());
    assert_eq!(parsed.queues, QueueConfig::default());
}

#[test]
fn typo_in_a_field_is_rejected() {
    let parsed = serde_json::from_str::<CameraEngineConfig>(r#"{"recovery":{"max_attempts":3}}"#);
    assert!(parsed.is_err(), "опечатка не должна молча превращаться в дефолт");
}

#[test]
fn each_broken_value_gives_its_own_error() {
    let mut config = CameraEngineConfig::default();
    config.recovery.max_consecutive_attempts = 0;
    assert_eq!(config.validate(), Err(CameraEngineConfigError::NoRecoveryAttempts));

    let mut config = CameraEngineConfig::default();
    config.recovery.stable_frames_before_reset = 0;
    assert_eq!(config.validate(), Err(CameraEngineConfigError::NoStableFrames));

    let mut config = CameraEngineConfig::default();
    config.recovery.retry_backoff_ms.clear();
    assert_eq!(config.validate(), Err(CameraEngineConfigError::EmptyBackoff));

    let mut config = CameraEngineConfig::default();
    config.recovery.retry_backoff_ms = vec![100, CameraEngineConfig::MAX_BACKOFF_MS + 1];
    assert_eq!(
        config.validate(),
        Err(CameraEngineConfigError::BackoffTooLong(
            CameraEngineConfig::MAX_BACKOFF_MS + 1
        ))
    );

    let mut config = CameraEngineConfig::default();
    config.roles.tele_max_fov_degrees = 120.0;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidRoleThresholds { .. })
    ));

    let mut config = CameraEngineConfig::default();
    config.cadence.gap_ratio = 1.0;
    assert_eq!(config.validate(), Err(CameraEngineConfigError::InvalidGapRatio(1.0)));

    let mut config = CameraEngineConfig::default();
    config.queues.capture_event_capacity = 0;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidQueueCapacity { .. })
    ));

    for broken in [0, CameraEngineConfig::MAX_DROP_SHUTDOWN_TIMEOUT_MS + 1] {
        let mut config = CameraEngineConfig::default();
        config.runtime.drop_shutdown_timeout_ms = broken;
        assert_eq!(
            config.validate(),
            Err(CameraEngineConfigError::InvalidDropShutdownTimeout {
                value: broken,
                max: CameraEngineConfig::MAX_DROP_SHUTDOWN_TIMEOUT_MS,
            })
        );
    }
}

#[test]
fn backoff_schedule_repeats_its_last_element() {
    let config = CameraEngineConfig::default();
    assert_eq!(config.backoff_for_attempt(1), 200);
    assert_eq!(config.backoff_for_attempt(2), 500);
    assert_eq!(config.backoff_for_attempt(4), 2_000);
    assert_eq!(config.backoff_for_attempt(40), 2_000);
    // Номер попытки 0 трактуется как первая, а не как выход за массив.
    assert_eq!(config.backoff_for_attempt(0), 200);
}
