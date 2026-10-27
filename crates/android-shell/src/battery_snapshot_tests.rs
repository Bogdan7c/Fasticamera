//! Тесты разбора снимка батареи: каждый слот, маркеры «не поддержано» и ошибки раскладки.

use shell_contract::power::{BatteryCurrent, ChargingState, CurrentNowUnit, MeasurementValidity, PowerPlug};

use super::*;

const LONG_UNSUPPORTED: i64 = i64::MIN;
const INT_UNSUPPORTED: i64 = i32::MIN as i64;

/// Снимок «на батарее» как его отдал бы OnePlus 11 (ток со знаком по документации Android).
fn on_battery_slots() -> [i64; slot::COUNT] {
    let mut slots = [0_i64; slot::COUNT];
    slots[slot::LAYOUT_VERSION] = BATTERY_SNAPSHOT_LAYOUT_VERSION;
    slots[slot::SAMPLED_AT_MONOTONIC_NANOS] = 123_456_789_000;
    slots[slot::CURRENT_NOW_MICROAMPS] = -612_000;
    slots[slot::CHARGE_COUNTER_MICROAMP_HOURS] = 3_912_000;
    slots[slot::STATUS] = 3; // BATTERY_STATUS_DISCHARGING
    slots[slot::PLUGGED] = 0;
    slots[slot::TEMPERATURE_DECI_CELSIUS] = 318;
    slots[slot::LEVEL] = 73;
    slots[slot::SCALE] = 100;
    slots[slot::VOLTAGE_MILLIVOLTS] = 4_012;
    slots[slot::IS_CHARGING] = 0;
    slots
}

fn decode(slots: &[i64]) -> BatterySample {
    decode_battery_snapshot(slots, CurrentNowUnit::Microamps).expect("снимок разбирается")
}

#[test]
fn full_snapshot_maps_every_slot_to_its_field() {
    let sample = decode(&on_battery_slots());
    assert_eq!(sample.at.as_nanos(), 123_456_789_000);
    assert_eq!(sample.current, Some(BatteryCurrent::from_raw_microamps(-612_000)));
    assert_eq!(
        sample.charge_counter.map(|charge| charge.microamp_hours()),
        Some(3_912_000)
    );
    assert_eq!(sample.charging, ChargingState::Discharging);
    assert_eq!(sample.plug, PowerPlug::Unplugged);
    assert_eq!(sample.temperature.map(|value| value.deci_celsius()), Some(318));
    assert_eq!(sample.level_percent, Some(73));
    assert_eq!(sample.voltage.map(|value| value.millivolts()), Some(4_012));
    assert_eq!(sample.measurement_validity(), MeasurementValidity::Valid);
}

#[test]
fn unsupported_markers_become_none_not_zero() {
    let mut slots = on_battery_slots();
    slots[slot::CURRENT_NOW_MICROAMPS] = LONG_UNSUPPORTED;
    slots[slot::CHARGE_COUNTER_MICROAMP_HOURS] = INT_UNSUPPORTED;
    slots[slot::TEMPERATURE_DECI_CELSIUS] = INT_UNSUPPORTED;
    slots[slot::LEVEL] = INT_UNSUPPORTED;
    slots[slot::VOLTAGE_MILLIVOLTS] = INT_UNSUPPORTED;
    let sample = decode(&slots);
    assert_eq!(sample.current, None);
    assert_eq!(sample.charge_counter, None);
    assert_eq!(sample.temperature, None);
    assert_eq!(sample.level_percent, None);
    assert_eq!(sample.voltage, None);

    let mut int_path = on_battery_slots();
    int_path[slot::CURRENT_NOW_MICROAMPS] = INT_UNSUPPORTED;
    int_path[slot::CHARGE_COUNTER_MICROAMP_HOURS] = LONG_UNSUPPORTED;
    let sample = decode(&int_path);
    assert_eq!(sample.current, None, "Integer.MIN_VALUE тоже значит «не поддержано»");
    assert_eq!(sample.charge_counter, None);
}

#[test]
fn zero_current_is_a_real_value() {
    let mut slots = on_battery_slots();
    slots[slot::CURRENT_NOW_MICROAMPS] = 0;
    assert_eq!(decode(&slots).current, Some(BatteryCurrent::from_raw_microamps(0)));
}

#[test]
fn missing_sticky_intent_makes_power_state_unknown_and_measurement_refused() {
    let mut slots = on_battery_slots();
    for index in [
        slot::STATUS,
        slot::PLUGGED,
        slot::TEMPERATURE_DECI_CELSIUS,
        slot::LEVEL,
        slot::SCALE,
        slot::VOLTAGE_MILLIVOLTS,
    ] {
        slots[index] = INT_UNSUPPORTED;
    }
    let sample = decode(&slots);
    assert_eq!(sample.plug, PowerPlug::Unknown);
    assert_eq!(sample.charging, ChargingState::Unknown);
    assert_eq!(
        sample.measurement_validity(),
        MeasurementValidity::RejectedPowerStateUnknown {
            charging: ChargingState::Unknown
        }
    );
}

#[test]
fn every_plug_code_is_decoded_and_external_power_is_refused() {
    let cases = [
        (1, PowerPlug::Ac),
        (2, PowerPlug::Usb),
        (4, PowerPlug::Wireless),
        (8, PowerPlug::Dock),
    ];
    for (code, expected) in cases {
        let mut slots = on_battery_slots();
        slots[slot::PLUGGED] = code;
        slots[slot::STATUS] = 2; // BATTERY_STATUS_CHARGING
        let sample = decode(&slots);
        assert_eq!(sample.plug, expected, "код {code}");
        assert_eq!(
            sample.measurement_validity(),
            MeasurementValidity::RejectedExternalPower {
                plug: expected,
                charging: ChargingState::Charging
            }
        );
    }
    let mut unknown_code = on_battery_slots();
    unknown_code[slot::PLUGGED] = 3; // битовая смесь AC|USB или будущий тип
    assert_eq!(decode(&unknown_code).plug, PowerPlug::Unknown);
}

#[test]
fn every_status_code_is_decoded() {
    let cases = [
        (1, ChargingState::Unknown),
        (2, ChargingState::Charging),
        (3, ChargingState::Discharging),
        (4, ChargingState::NotCharging),
        (5, ChargingState::Full),
        (42, ChargingState::Unknown),
    ];
    for (code, expected) in cases {
        let mut slots = on_battery_slots();
        slots[slot::STATUS] = code;
        assert_eq!(decode(&slots).charging, expected, "код {code}");
    }
}

#[test]
fn platform_is_charging_overrides_stale_status() {
    let mut slots = on_battery_slots();
    slots[slot::IS_CHARGING] = 1;
    let sample = decode(&slots);
    assert_eq!(sample.charging, ChargingState::Charging);
    // Даже без признака кабеля замер откажется: энергия идёт в батарею.
    assert!(matches!(
        sample.measurement_validity(),
        MeasurementValidity::RejectedExternalPower { .. }
    ));
    let mut full = on_battery_slots();
    full[slot::STATUS] = 5;
    full[slot::IS_CHARGING] = 1;
    assert_eq!(
        decode(&full).charging,
        ChargingState::Full,
        "Full не превращается в Charging"
    );
}

#[test]
fn level_percent_rounds_and_respects_scale() {
    let cases = [
        ((73, 100), Some(73)),
        ((1, 3), Some(33)),
        ((2, 3), Some(67)),
        ((150, 100), Some(100)),
        ((-1, 100), None),
        ((50, 0), None),
    ];
    for ((level, scale), expected) in cases {
        let mut slots = on_battery_slots();
        slots[slot::LEVEL] = level;
        slots[slot::SCALE] = scale;
        assert_eq!(decode(&slots).level_percent, expected, "{level}/{scale}");
    }
}

#[test]
fn nonpositive_voltage_is_none() {
    let mut slots = on_battery_slots();
    slots[slot::VOLTAGE_MILLIVOLTS] = -1;
    assert_eq!(decode(&slots).voltage, None);
    slots[slot::VOLTAGE_MILLIVOLTS] = 0;
    assert_eq!(decode(&slots).voltage, None);
}

#[test]
fn wrong_slot_count_is_reported() {
    let slots = on_battery_slots();
    assert_eq!(
        decode_battery_snapshot(&slots[..slot::COUNT - 1], CurrentNowUnit::Microamps),
        Err(BatterySnapshotError::WrongSlotCount {
            expected: slot::COUNT,
            actual: slot::COUNT - 1
        })
    );
    let mut longer = slots.to_vec();
    longer.push(0);
    assert_eq!(
        decode_battery_snapshot(&longer, CurrentNowUnit::Microamps),
        Err(BatterySnapshotError::wrong_slot_count(slot::COUNT + 1))
    );
}

#[test]
fn foreign_layout_version_is_rejected() {
    let mut slots = on_battery_slots();
    slots[slot::LAYOUT_VERSION] = 2;
    assert_eq!(
        decode_battery_snapshot(&slots, CurrentNowUnit::Microamps),
        Err(BatterySnapshotError::UnsupportedLayoutVersion {
            expected: BATTERY_SNAPSHOT_LAYOUT_VERSION,
            actual: 2
        })
    );
}

#[test]
fn int_slot_outside_java_int_is_a_layout_error() {
    let mut slots = on_battery_slots();
    slots[slot::TEMPERATURE_DECI_CELSIUS] = i64::from(i32::MAX) + 1;
    assert_eq!(
        decode_battery_snapshot(&slots, CurrentNowUnit::Microamps),
        Err(BatterySnapshotError::IntSlotOutOfRange {
            slot: "temperature",
            value: i64::from(i32::MAX) + 1
        })
    );
}

#[test]
fn milliamp_kernel_value_is_converted_to_microamps_before_the_contract() {
    // OnePlus 11 (OxygenOS 16, факт 07): CURRENT_NOW ≈ 490 при работающей камере — это мА.
    let mut slots = on_battery_slots();
    slots[slot::CURRENT_NOW_MICROAMPS] = 490;
    let as_milliamps = decode_battery_snapshot(&slots, CurrentNowUnit::Milliamps).expect("снимок разбирается");
    assert_eq!(as_milliamps.current, Some(BatteryCurrent::from_raw_microamps(490_000)));
    let as_documented = decode_battery_snapshot(&slots, CurrentNowUnit::Microamps).expect("снимок разбирается");
    assert_eq!(as_documented.current, Some(BatteryCurrent::from_raw_microamps(490)));
    // Маркер «не поддерживается» в мА не превращается в число.
    slots[slot::CURRENT_NOW_MICROAMPS] = i64::MIN;
    let unsupported = decode_battery_snapshot(&slots, CurrentNowUnit::Milliamps).expect("снимок разбирается");
    assert_eq!(unsupported.current, None);
}
