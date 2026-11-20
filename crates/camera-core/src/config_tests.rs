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

#[test]
fn fallback_defaults_to_30_and_validates_its_rate() {
    assert_eq!(
        CameraEngineConfig::default().stream.frame_rate_fallback,
        FrameRateFallback::To { fps: 30 }
    );
    let mut config = CameraEngineConfig::default();
    config.stream.frame_rate_fallback = FrameRateFallback::Disabled;
    assert_eq!(config.validate(), Ok(()));
    for broken in [0, TargetFrameRate::MAX + 1] {
        config.stream.frame_rate_fallback = FrameRateFallback::To { fps: broken };
        assert_eq!(
            config.validate(),
            Err(CameraEngineConfigError::InvalidFrameRateFallback(broken))
        );
    }
    let parsed: CameraEngineConfig =
        serde_json::from_str(r#"{"stream":{"frame_rate_fallback":"disabled"}}"#).expect("отключённый откат");
    assert_eq!(parsed.stream.frame_rate_fallback, FrameRateFallback::Disabled);
    let parsed: CameraEngineConfig =
        serde_json::from_str(r#"{"stream":{"frame_rate_fallback":{"to":{"fps":24}}}}"#).expect("откат на 24");
    assert_eq!(parsed.stream.frame_rate_fallback, FrameRateFallback::To { fps: 24 });
}

#[test]
fn three_a_defaults_are_valid_and_each_broken_value_gives_its_own_error() {
    let defaults = CameraEngineConfig::default().three_a;
    assert!(
        defaults.reset_point_on_camera_switch,
        "по умолчанию точка сбрасывается при смене камеры"
    );
    assert_eq!(defaults.max_bracket_frames, 4);

    for broken in [0.0, -0.1, 1.5, f32::NAN] {
        let mut config = CameraEngineConfig::default();
        config.three_a.metering_region_size_fraction = broken;
        assert!(
            matches!(
                config.validate(),
                Err(CameraEngineConfigError::InvalidMeteringRegionSize(_))
            ),
            "размер региона {broken} должен отвергаться"
        );
    }

    let mut config = CameraEngineConfig::default();
    config.three_a.max_bracket_frames = 0;
    assert_eq!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidThreeALimit {
            field: "max_bracket_frames",
            value: 0,
            max: CameraEngineConfig::MAX_THREE_A_BRACKET_FRAMES,
        })
    );

    let mut config = CameraEngineConfig::default();
    config.three_a.focus_settle_timeout_ms = CameraEngineConfig::MAX_THREE_A_WAIT_MS + 1;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidThreeALimit {
            field: "focus_settle_timeout_ms",
            ..
        })
    ));

    let parsed = serde_json::from_str::<CameraEngineConfig>(r#"{"three_a":{"focus_timeout":5}}"#);
    assert!(
        parsed.is_err(),
        "опечатка в секции three_a не должна молча стать дефолтом"
    );
}

#[test]
fn own_exposure_section_parses_validates_and_rejects_nonsense() {
    use crate::three_a_own::FlickerPolicy;

    let parsed: CameraEngineConfig =
        serde_json::from_str(r#"{"three_a":{"exposure":{"flicker":"always_protect","replan_min_change_ev":0.1}}}"#)
            .expect("частичная секция");
    assert_eq!(parsed.three_a.exposure.flicker, FlickerPolicy::AlwaysProtect);
    assert_eq!(parsed.three_a.exposure.replan_min_change_ev, 0.1);
    assert_eq!(
        parsed.three_a.exposure.initial_exposure_seconds_iso, 4.0,
        "остальное — по умолчанию"
    );
    assert_eq!(parsed.validate(), Ok(()));
    assert_eq!(
        FlickerPolicy::default(),
        FlickerPolicy::NeverProtect,
        "по умолчанию защита выключена (решение 77)"
    );
    assert_eq!(
        CameraEngineConfig::default().three_a.exposure.flicker,
        FlickerPolicy::NeverProtect
    );

    // Автодетект убран (решение 77): старая форма отвергается, а не молча превращается в другую.
    let legacy = r#"{"three_a":{"exposure":{"flicker":{"policy":"follow_detector","clear_after_frames":12}}}}"#;
    assert!(
        serde_json::from_str::<CameraEngineConfig>(legacy).is_err(),
        "старый конфиг должен отвергаться: {legacy}"
    );

    // Старый объект с лишним полем рядом с ручной политикой тоже не проходит: политика — только строка.
    for legacy_object in [
        r#"{"three_a":{"exposure":{"flicker":{"policy":"always_protect","clear_after_frames":12}}}}"#,
        r#"{"three_a":{"exposure":{"flicker":{"policy":"always_protect"}}}}"#,
        r#"{"three_a":{"exposure":{"flicker":"50hz"}}}"#,
    ] {
        assert!(
            serde_json::from_str::<CameraEngineConfig>(legacy_object).is_err(),
            "должно отвергаться: {legacy_object}"
        );
    }
    let never: CameraEngineConfig =
        serde_json::from_str(r#"{"three_a":{"exposure":{"flicker":"never_protect"}}}"#).expect("ручное «выкл»");
    assert_eq!(never.three_a.exposure.flicker, FlickerPolicy::NeverProtect);

    let typo = serde_json::from_str::<CameraEngineConfig>(r#"{"three_a":{"exposure":{"flicker_policy":"x"}}}"#);
    assert!(typo.is_err(), "опечатка в секции exposure отвергается");

    let mut config = CameraEngineConfig::default();
    config.three_a.exposure.initial_exposure_seconds_iso = 0.0;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidExposureSetting {
            field: "initial_exposure_seconds_iso",
            ..
        })
    ));
    for tolerance in [0.0, -1.0, f32::NAN, 5.0] {
        let mut config = CameraEngineConfig::default();
        config.three_a.exposure.not_reached_tolerance_ev = tolerance;
        assert!(
            matches!(
                config.validate(),
                Err(CameraEngineConfigError::InvalidExposureSetting {
                    field: "not_reached_tolerance_ev",
                    ..
                })
            ),
            "допуск {tolerance}"
        );
    }
    for step in [0.0, -0.1, f32::NAN] {
        let mut config = CameraEngineConfig::default();
        config.three_a.exposure.shortfall_report_step_ev = step;
        assert!(
            matches!(
                config.validate(),
                Err(CameraEngineConfigError::InvalidExposureSetting {
                    field: "shortfall_report_step_ev",
                    ..
                })
            ),
            "шаг {step}"
        );
    }
    let mut config = CameraEngineConfig::default();
    config.three_a.exposure.replan_min_change_ev = f32::INFINITY;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidExposureSetting {
            field: "replan_min_change_ev",
            ..
        })
    ));
}

#[test]
fn engine_config_rejects_invalid_zoom_section_and_accepts_partial_json() {
    let mut config = CameraEngineConfig::default();
    config.zoom.front_max_ratio = f32::NAN;
    assert!(matches!(
        config.validate(),
        Err(CameraEngineConfigError::InvalidZoom(
            crate::zoom::ZoomConfigError::InvalidMaxRatio {
                field: "front_max_ratio",
                ..
            }
        ))
    ));
    let parsed: CameraEngineConfig = serde_json::from_str(r#"{"zoom": {"back_max_ratio": 8.0}}"#).expect("json");
    assert_eq!(parsed.zoom.back_max_ratio, 8.0);
    assert_eq!(parsed.validate(), Ok(()));
}
