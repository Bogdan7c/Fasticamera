use super::*;

#[test]
fn valid_config_builds_the_stream_from_it_without_complaints() {
    let config = RawFrameStreamConfig {
        pairing_timeout_ms: 77,
        ..RawFrameStreamConfig::default()
    };
    let creation = create_raw_stream(&config);
    assert_eq!(creation.rejected_config, None);
    assert_eq!(creation.effective_config.pairing_timeout_ms, 77);
}

#[test]
fn invalid_config_does_not_stop_the_stream_but_is_reported_and_replaced_by_defaults() {
    let config = RawFrameStreamConfig {
        max_pending_frames: 0,
        ..RawFrameStreamConfig::default()
    };
    let creation = create_raw_stream(&config);
    let rejected = creation.rejected_config.expect("отказ конфига возвращён");
    assert!(rejected.to_string().contains("max_pending_frames"), "{rejected}");
    assert_eq!(creation.effective_config, RawFrameStreamConfig::default());
}

#[test]
fn observer_sees_exactly_what_the_producer_end_of_the_same_stream_writes() {
    let creation = create_raw_stream(&RawFrameStreamConfig::default());
    let probe = CpuAccessProbe::disarmed();
    let services = observe_stream(&creation, probe.clone());
    assert_eq!(services.counters().cpu_copies, 0);

    // Камера (производитель) отметила копию через CPU — сценарий обязан её увидеть.
    creation.stream.producer().note_cpu_copy();
    creation.stream.producer().note_cpu_copy();
    assert_eq!(services.counters().cpu_copies, 2);

    // Рубильник пробы общий с тем, что держит бэкенд.
    services.arm_cpu_probe();
    assert!(probe.is_armed());
    services.disarm_cpu_probe();
    assert!(!probe.is_armed());
}

#[test]
fn services_plan_the_pool_from_the_effective_config_not_the_rejected_one() {
    let creation = create_raw_stream(&RawFrameStreamConfig {
        pool_slack: 99,
        ..RawFrameStreamConfig::default()
    });
    let services = observe_stream(&creation, CpuAccessProbe::disarmed());
    assert_eq!(
        services.pool_plan_upper_bound().slack,
        RawFrameStreamConfig::default().pool_slack
    );
}
