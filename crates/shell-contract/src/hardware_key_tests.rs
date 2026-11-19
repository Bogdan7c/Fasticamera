use super::*;

fn event(phase: KeyPhase, repeat: KeyRepeat) -> HardwareKeyEvent {
    HardwareKeyEvent {
        key: HardwareKey::VolumeDown,
        phase,
        repeat,
        time: MonotonicNanos::from_nanos(7),
    }
}

#[test]
fn only_first_press_is_initial_press() {
    assert!(event(KeyPhase::Pressed, KeyRepeat::Initial).is_initial_press());
    assert!(!event(KeyPhase::Pressed, KeyRepeat::AutoRepeat { count: 1 }).is_initial_press());
    assert!(!event(KeyPhase::Released, KeyRepeat::Initial).is_initial_press());
}

#[test]
fn serializes_with_snake_case_names_and_plain_time() {
    let json = serde_json::to_value(event(KeyPhase::Pressed, KeyRepeat::AutoRepeat { count: 3 })).expect("json");
    assert_eq!(
        json,
        serde_json::json!({
            "key": "volume_down",
            "phase": "pressed",
            "repeat": {"auto_repeat": {"count": 3}},
            "time": 7,
        })
    );
}
