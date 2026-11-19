use super::*;

fn raw(key_code: i32, action: i32, repeat_count: i32) -> RawKeyEvent {
    RawKeyEvent {
        key_code,
        action,
        repeat_count,
        event_time_ms: 5_000,
    }
}

fn handled(result: Result<DecodedKey, KeyDecodeError>) -> HardwareKeyEvent {
    match result {
        Ok(DecodedKey::Handled(event)) => event,
        other => panic!("ожидали обработанную клавишу, получили {other:?}"),
    }
}

#[test]
fn volume_up_first_press_converts_uptime_millis_to_monotonic_nanos() {
    let event = handled(decode_key_event(&raw(24, 0, 0)));
    assert_eq!(event.key, HardwareKey::VolumeUp);
    assert_eq!(event.phase, KeyPhase::Pressed);
    assert_eq!(event.repeat, KeyRepeat::Initial);
    assert_eq!(event.time, MonotonicNanos::from_nanos(5_000_000_000));
    assert!(event.is_initial_press());
}

#[test]
fn volume_down_release_is_released_initial() {
    let event = handled(decode_key_event(&raw(25, 1, 0)));
    assert_eq!(
        (event.key, event.phase, event.repeat),
        (HardwareKey::VolumeDown, KeyPhase::Released, KeyRepeat::Initial)
    );
}

#[test]
fn held_key_reports_auto_repeat_with_count() {
    let event = handled(decode_key_event(&raw(24, 0, 3)));
    assert_eq!(event.repeat, KeyRepeat::AutoRepeat { count: 3 });
    assert!(!event.is_initial_press());
}

#[test]
fn release_ignores_repeat_count() {
    let event = handled(decode_key_event(&raw(24, 1, 4)));
    assert_eq!(event.repeat, KeyRepeat::Initial);
}

#[test]
fn other_keys_are_left_to_the_system() {
    // KEYCODE_BACK = 4, KEYCODE_POWER = 26 (не наши).
    assert_eq!(
        decode_key_event(&raw(4, 0, 0)),
        Ok(DecodedKey::NotOurKey { key_code: 4 })
    );
    assert_eq!(
        decode_key_event(&raw(26, 0, 0)),
        Ok(DecodedKey::NotOurKey { key_code: 26 })
    );
}

#[test]
fn unknown_action_is_an_explicit_error() {
    // ACTION_MULTIPLE = 2 — не нажатие и не отпускание.
    assert_eq!(
        decode_key_event(&raw(24, 2, 0)),
        Err(KeyDecodeError::UnknownAction {
            key_code: 24,
            action: 2
        })
    );
}

#[test]
fn negative_repeat_count_is_an_explicit_error() {
    assert_eq!(
        decode_key_event(&raw(25, 0, -1)),
        Err(KeyDecodeError::NegativeRepeatCount {
            key_code: 25,
            repeat_count: -1
        })
    );
}

#[test]
fn huge_time_saturates_instead_of_overflowing() {
    let mut event = raw(24, 0, 0);
    event.event_time_ms = i64::MAX;
    assert_eq!(
        handled(decode_key_event(&event)).time,
        MonotonicNanos::from_nanos(i64::MAX)
    );
}

#[test]
fn only_a_delivered_key_is_consumed_otherwise_it_stays_with_the_system() {
    assert_eq!(
        KeyConsumption::after_send(SendOutcome::Delivered),
        KeyConsumption::Consumed
    );
    assert!(KeyConsumption::after_send(SendOutcome::Delivered).is_consumed());
    for lost in [
        SendOutcome::CriticalLost,
        SendOutcome::ReceiverGone,
        SendOutcome::TouchDropped,
    ] {
        let decision = KeyConsumption::after_send(lost);
        assert_eq!(decision, KeyConsumption::LeftToSystem, "{lost:?}");
        assert!(!decision.is_consumed(), "{lost:?}");
    }
}
