//! Вердикт `camera-evict`: проходной образец и каждое правило отказа.

use super::*;

fn healthy() -> EvictObservation {
    EvictObservation {
        failure: None,
        camera: Some("2".to_owned()),
        streaming_after_ms: Some(700),
        unavailable: Some(UnavailableSeen {
            reason: CameraUnavailableReason::Evicted,
            at_ms: 6_000,
        }),
        further_unavailable: Vec::new(),
        recovered: Some(RecoveredSeen {
            reason: CameraUnavailableReason::Evicted,
            attempts: 1,
            at_ms: 9_500,
            recovery_ms: 3_500,
        }),
        final_state: "streaming",
        counters: CameraCounters {
            recovery_attempts: 1,
            recoveries: 1,
            ..CameraCounters::default()
        },
        events: Vec::new(),
        events_dropped: 0,
    }
}

fn violations_with(change: impl FnOnce(&mut EvictObservation)) -> Vec<String> {
    let mut observation = healthy();
    change(&mut observation);
    evaluate_evict(&observation)
}

#[test]
fn healthy_eviction_and_recovery_pass() {
    assert_eq!(violations_with(|_| {}), Vec::<String>::new());
}

#[test]
fn all_accepted_eviction_reasons_pass_and_others_fail_naming_the_reason() {
    for reason in ACCEPTED_EVICTION_REASONS {
        let violations = violations_with(|o| {
            o.unavailable = Some(UnavailableSeen { reason, at_ms: 1 });
        });
        assert_eq!(violations, Vec::<String>::new(), "{reason:?}");
    }
    let unexpected = violations_with(|o| {
        o.unavailable = Some(UnavailableSeen {
            reason: CameraUnavailableReason::DeviceFailure,
            at_ms: 1,
        });
    });
    assert_eq!(unexpected.len(), 1, "{unexpected:?}");
    assert!(
        unexpected[0].contains("device_failure") && unexpected[0].contains("evicted"),
        "{unexpected:?}"
    );
}

#[test]
fn no_eviction_observed_is_a_violation_but_a_failure_is_not_double_reported() {
    let silent = violations_with(|o| {
        o.unavailable = None;
        o.recovered = None;
        o.final_state = "streaming";
    });
    assert!(
        silent.iter().any(|text| text.contains("вытеснение не наблюдалось")),
        "{silent:?}"
    );

    let failed = violations_with(|o| {
        o.failure = Some(RunFailure::NeverEvicted {
            last_state: "streaming",
        });
        o.unavailable = None;
        o.recovered = None;
    });
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("вытеснения не было"), "{failed:?}");
}

#[test]
fn missing_recovery_event_wrong_final_state_and_zero_recoveries_are_violations() {
    let no_event = violations_with(|o| o.recovered = None);
    assert!(no_event.iter().any(|text| text.contains("Recovered")), "{no_event:?}");
    let wrong_state = violations_with(|o| o.final_state = "error");
    assert!(
        wrong_state
            .iter()
            .any(|text| text.contains("не идёт") && text.contains("error")),
        "{wrong_state:?}"
    );
    let no_counter = violations_with(|o| o.counters.recoveries = 0);
    assert!(
        no_counter.iter().any(|text| text.contains("recoveries")),
        "{no_counter:?}"
    );
}

#[test]
fn never_recovered_failure_is_described() {
    let violations = violations_with(|o| {
        o.failure = Some(RunFailure::NeverRecovered { last_state: "error" });
        o.recovered = None;
    });
    assert_eq!(violations.len(), 1, "{violations:?}");
    assert!(
        violations[0].contains("не восстановился") && violations[0].contains("error"),
        "{violations:?}"
    );
}

#[test]
fn report_shape_carries_reason_timings_and_engine_counters() {
    let json = serde_json::to_value(healthy()).expect("json");
    assert_eq!(json["unavailable"]["reason"], "evicted");
    assert_eq!(json["recovered"]["recovery_ms"], 3_500);
    assert_eq!(json["counters"]["recovery_attempts"], 1);
    assert_eq!(json["counters"]["recoveries"], 1);
    assert_eq!(json["final_state"], "streaming");
}
