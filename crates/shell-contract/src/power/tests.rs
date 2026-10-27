//! Тесты контракта питания: знак тока, годность замера, кольцо и fake-источник.

use std::num::NonZeroUsize;
use std::time::Duration;

use super::fake::{FakePowerError, FakePowerSource};
use super::*;

fn sample_at(nanos: i64, raw_current: i64, plug: PowerPlug, charging: ChargingState) -> BatterySample {
    BatterySample {
        at: MonotonicNanos::from_nanos(nanos),
        current: Some(BatteryCurrent::from_raw_microamps(raw_current)),
        charge_counter: Some(MicroAmpHours::from_microamp_hours(4_000_000 - nanos)),
        charging,
        plug,
        temperature: Some(DeciCelsius::from_deci_celsius(312)),
        level_percent: Some(80),
        voltage: Some(MilliVolts::from_millivolts(4_100)),
    }
}

fn on_battery(nanos: i64, raw_current: i64) -> BatterySample {
    sample_at(nanos, raw_current, PowerPlug::Unplugged, ChargingState::Discharging)
}

fn capacity(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).expect("ненулевая ёмкость")
}

fn request(ring_capacity: usize) -> PowerSamplingRequest {
    PowerSamplingRequest {
        current_now_unit: CurrentNowUnit::Microamps,
        period: PowerSamplingPeriod::new(Duration::from_millis(250)).expect("ненулевой период"),
        ring_capacity: capacity(ring_capacity),
    }
}

#[test]
fn discharge_current_follows_declared_sign_convention() {
    let documented = BatteryCurrent::from_raw_microamps(-450_000);
    assert_eq!(
        documented
            .discharge_current(DischargeSign::NegativeWhenDischarging)
            .microamps(),
        450_000
    );
    let inverted = BatteryCurrent::from_raw_microamps(450_000);
    assert_eq!(
        inverted
            .discharge_current(DischargeSign::PositiveWhenDischarging)
            .microamps(),
        450_000
    );
    // Неверное соглашение даёт отрицательный «разряд» — потребитель это видит, а не получает модуль.
    assert_eq!(
        inverted
            .discharge_current(DischargeSign::NegativeWhenDischarging)
            .microamps(),
        -450_000
    );
    assert_eq!(documented.raw_microamps(), -450_000, "сырое значение не меняется");
}

#[test]
fn discharge_current_of_extreme_raw_value_saturates_instead_of_panicking() {
    let extreme = BatteryCurrent::from_raw_microamps(i64::MIN);
    assert_eq!(
        extreme
            .discharge_current(DischargeSign::NegativeWhenDischarging)
            .microamps(),
        i64::MAX
    );
}

#[test]
fn discharge_current_converts_to_milliamps() {
    let current =
        BatteryCurrent::from_raw_microamps(-1_234_500).discharge_current(DischargeSign::NegativeWhenDischarging);
    assert!((current.milliamps() - 1_234.5).abs() < 1e-9);
}

#[test]
fn only_battery_power_is_valid_for_measurement() {
    let valid_states = [
        ChargingState::Discharging,
        ChargingState::NotCharging,
        ChargingState::Full,
        ChargingState::Unknown,
    ];
    for charging in valid_states {
        let sample = sample_at(1, -400_000, PowerPlug::Unplugged, charging);
        assert_eq!(
            sample.measurement_validity(),
            MeasurementValidity::Valid,
            "{charging:?}"
        );
    }
}

#[test]
fn every_external_plug_rejects_measurement_with_reason() {
    let plugs = [PowerPlug::Ac, PowerPlug::Usb, PowerPlug::Wireless, PowerPlug::Dock];
    let states = [
        ChargingState::Charging,
        ChargingState::Discharging,
        ChargingState::NotCharging,
        ChargingState::Full,
        ChargingState::Unknown,
    ];
    for plug in plugs {
        for charging in states {
            let sample = sample_at(1, 500_000, plug, charging);
            assert_eq!(
                sample.measurement_validity(),
                MeasurementValidity::RejectedExternalPower { plug, charging },
                "{plug:?} / {charging:?}"
            );
        }
    }
}

#[test]
fn charging_status_without_plug_is_rejected_as_contradiction() {
    let sample = sample_at(1, 500_000, PowerPlug::Unplugged, ChargingState::Charging);
    assert_eq!(
        sample.measurement_validity(),
        MeasurementValidity::RejectedExternalPower {
            plug: PowerPlug::Unplugged,
            charging: ChargingState::Charging,
        }
    );
}

#[test]
fn unknown_plug_is_rejected_as_unverifiable() {
    let sample = sample_at(1, -400_000, PowerPlug::Unknown, ChargingState::Discharging);
    assert_eq!(
        sample.measurement_validity(),
        MeasurementValidity::RejectedPowerStateUnknown {
            charging: ChargingState::Discharging
        }
    );
}

#[test]
fn sign_detection_uses_only_valid_nonzero_samples() {
    let samples = [
        on_battery(1, -410_000),
        on_battery(2, 0),
        on_battery(3, -390_000),
        // На зарядке ток положительный — не должен ломать определение знака разряда.
        sample_at(4, 900_000, PowerPlug::Usb, ChargingState::Charging),
        on_battery(5, -420_000),
    ];
    assert_eq!(
        detect_discharge_sign(&samples, rule(3, 1.0)),
        DischargeSignEvidence::Detected {
            sign: DischargeSign::NegativeWhenDischarging,
            samples: 3,
            opposing: 0
        }
    );
    let inverted = [on_battery(1, 410_000), on_battery(2, 380_000)];
    assert_eq!(
        detect_discharge_sign(&inverted, rule(2, 1.0)),
        DischargeSignEvidence::Detected {
            sign: DischargeSign::PositiveWhenDischarging,
            samples: 2,
            opposing: 0
        }
    );
}

#[test]
fn sign_detection_refuses_to_guess() {
    let mixed = [on_battery(1, -410_000), on_battery(2, 20_000), on_battery(3, -400_000)];
    assert_eq!(
        detect_discharge_sign(&mixed, rule(2, 0.95)),
        DischargeSignEvidence::Ambiguous {
            positive: 1,
            negative: 2
        }
    );
    let too_few = [on_battery(1, -410_000), on_battery(2, 0)];
    assert_eq!(
        detect_discharge_sign(&too_few, rule(3, 0.95)),
        DischargeSignEvidence::InsufficientSamples { usable: 1, required: 3 }
    );
}

#[test]
fn charge_counter_difference_is_consumed_charge() {
    let start = MicroAmpHours::from_microamp_hours(3_500_000);
    let end = MicroAmpHours::from_microamp_hours(3_420_000);
    assert_eq!(end.consumed_since(start), 80_000);
    assert_eq!(start.consumed_since(end), -80_000, "заряд вырос — отрицательный расход");
}

#[test]
fn temperature_converts_tenths_to_degrees() {
    assert!((DeciCelsius::from_deci_celsius(312).celsius() - 31.2).abs() < 1e-5);
    assert!((DeciCelsius::from_deci_celsius(-55).celsius() + 5.5).abs() < 1e-5);
}

#[test]
fn zero_sampling_period_is_rejected() {
    assert_eq!(PowerSamplingPeriod::new(Duration::ZERO), None);
    assert_eq!(
        PowerSamplingPeriod::new(Duration::from_millis(500)).map(PowerSamplingPeriod::duration),
        Some(Duration::from_millis(500))
    );
}

#[test]
fn ring_drains_in_arrival_order_and_keeps_latest() {
    let ring = PowerSampleRing::new(capacity(4));
    for nanos in 1..=3 {
        assert_eq!(ring.push(on_battery(nanos, -400_000 - nanos)), PowerPushOutcome::Stored);
    }
    let mut drained = Vec::new();
    assert_eq!(ring.drain_into(&mut drained), 3);
    let times: Vec<i64> = drained.iter().map(|sample| sample.at.as_nanos()).collect();
    assert_eq!(times, vec![1, 2, 3]);
    assert_eq!(ring.latest().map(|sample| sample.at.as_nanos()), Some(3));
    // Повторный drain пуст, но не стирает уже забранное у вызывающего.
    assert_eq!(ring.drain_into(&mut drained), 0);
    assert_eq!(drained.len(), 3);
    assert_eq!(ring.stats().drained, 3);
}

#[test]
fn ring_overflow_evicts_oldest_and_counts_it() {
    let ring = PowerSampleRing::new(capacity(2));
    ring.push(on_battery(1, -1));
    ring.push(on_battery(2, -2));
    assert_eq!(ring.push(on_battery(3, -3)), PowerPushOutcome::StoredEvictingOldest);
    assert_eq!(ring.push(on_battery(4, -4)), PowerPushOutcome::StoredEvictingOldest);
    let mut drained = Vec::new();
    ring.drain_into(&mut drained);
    let times: Vec<i64> = drained.iter().map(|sample| sample.at.as_nanos()).collect();
    assert_eq!(times, vec![3, 4], "остались самые новые");
    let stats = ring.stats();
    assert_eq!(stats.stored, 4);
    assert_eq!(stats.evicted_unread, 2);
    assert_eq!(stats.drained, 2);
    // Баланс: записано = отдано + вытеснено + в кольце (0).
    assert_eq!(stats.stored, stats.drained + stats.evicted_unread);
}

#[test]
fn failed_reads_are_counted_per_kind_and_last_one_is_kept() {
    let ring = PowerSampleRing::new(capacity(2));
    ring.record_failed_read(PowerReadFailure::NotAttached);
    ring.record_failed_read(PowerReadFailure::PlatformCallFailed {
        reason: "binder".to_owned(),
    });
    ring.record_failed_read(PowerReadFailure::MalformedSnapshot {
        reason: "слотов 3".to_owned(),
    });
    ring.record_failed_read(PowerReadFailure::NotAttached);
    let stats = ring.stats();
    assert_eq!(stats.failed_not_attached, 2);
    assert_eq!(stats.failed_platform_call, 1);
    assert_eq!(stats.failed_malformed_snapshot, 1);
    assert_eq!(stats.failed_reads(), 4);
    assert_eq!(stats.stored, 0, "сбой чтения не создаёт показаний");
    assert_eq!(ring.last_failure(), Some(PowerReadFailure::NotAttached));
    ring.record_missed_ticks(3);
    assert_eq!(ring.stats().missed_ticks, 3);
    assert_eq!(ring.stats().failed_reads(), 4, "пропуск узла — не сбой чтения");
}

#[test]
fn fake_source_delivers_samples_through_subscription() {
    let source = FakePowerSource::new();
    let sampling = source.start_sampling(request(8)).expect("fake запускается");
    assert_eq!(source.last_request(), Some(request(8)));
    source.push_sample(on_battery(10, -500_000)).expect("опрос идёт");
    source.push_sample(on_battery(20, -520_000)).expect("опрос идёт");
    source.fail_read(PowerReadFailure::NotAttached).expect("опрос идёт");
    let mut drained = Vec::new();
    assert_eq!(sampling.drain_samples(&mut drained), 2);
    assert_eq!(
        drained.last().and_then(|sample| sample.current),
        Some(BatteryCurrent::from_raw_microamps(-520_000))
    );
    assert_eq!(sampling.latest_sample().map(|sample| sample.at.as_nanos()), Some(20));
    assert_eq!(sampling.stats().failed_not_attached, 1);
    assert_eq!(sampling.last_failure(), Some(PowerReadFailure::NotAttached));
}

#[test]
fn fake_source_allows_one_subscription_and_stops_on_drop() {
    let source = FakePowerSource::new();
    let sampling = source.start_sampling(request(2)).expect("первый запуск");
    assert_eq!(
        source.start_sampling(request(2)).map(|_| ()),
        Err(PowerSourceError::AlreadySampling)
    );
    assert!(source.is_sampling());
    drop(sampling);
    assert!(!source.is_sampling());
    assert_eq!(source.push_sample(on_battery(1, -1)), Err(FakePowerError::NotSampling));
    assert!(source.start_sampling(request(2)).is_ok(), "после остановки можно снова");
}

#[test]
fn fake_source_reports_scripted_start_error_once() {
    let source = FakePowerSource::new();
    source.fail_next_start(PowerSourceError::NotAttached);
    assert_eq!(
        source.start_sampling(request(2)).map(|_| ()),
        Err(PowerSourceError::NotAttached)
    );
    assert!(!source.is_sampling());
    assert!(source.start_sampling(request(2)).is_ok());
}

#[test]
fn sample_serializes_units_as_plain_numbers() {
    let json = serde_json::to_value(on_battery(7, -401_000)).expect("json");
    assert_eq!(json["current"], serde_json::json!(-401_000));
    assert_eq!(json["temperature"], serde_json::json!(312));
    assert_eq!(json["plug"], serde_json::json!("unplugged"));
    assert_eq!(
        serde_json::to_value(MeasurementValidity::RejectedExternalPower {
            plug: PowerPlug::Usb,
            charging: ChargingState::Charging
        })
        .expect("json"),
        serde_json::json!({"validity": "rejected_external_power", "plug": "usb", "charging": "charging"})
    );
}

fn rule(required: usize, agreement: f64) -> SignDetectionRule {
    SignDetectionRule {
        required: capacity(required),
        agreement: SignAgreement::new(agreement).expect("доля в (0,5; 1]"),
    }
}

/// 39 из 40 показаний — разряд, одно — шум другого знака: при пороге 95 % знак установлен, а
/// ровное разделение и «почти половина» — по-прежнему `Ambiguous`.
#[test]
fn sign_detection_tolerates_rare_opposite_noise_by_a_documented_majority() {
    let mut noisy: Vec<BatterySample> = (0..39).map(|index| on_battery(index, -400_000)).collect();
    noisy.push(on_battery(39, 5_000));
    assert_eq!(
        detect_discharge_sign(&noisy, rule(20, 0.95)),
        DischargeSignEvidence::Detected {
            sign: DischargeSign::NegativeWhenDischarging,
            samples: 40,
            opposing: 1
        }
    );
    // Тот же набор при строгом согласии (100 %) — не угадываем.
    assert_eq!(
        detect_discharge_sign(&noisy, rule(20, 1.0)),
        DischargeSignEvidence::Ambiguous {
            positive: 1,
            negative: 39
        }
    );
    // 37 из 40 (92,5 %) не дотягивает до 95 %.
    let split: Vec<BatterySample> = (0..40)
        .map(|index| on_battery(index, if index < 37 { -400_000 } else { 5_000 }))
        .collect();
    assert_eq!(
        detect_discharge_sign(&split, rule(20, 0.95)),
        DischargeSignEvidence::Ambiguous {
            positive: 3,
            negative: 37
        }
    );
    // Ничья не угадывается ни при каком допустимом пороге.
    let tie = [on_battery(1, -400_000), on_battery(2, 400_000)];
    assert!(matches!(
        detect_discharge_sign(&tie, rule(2, 0.51)),
        DischargeSignEvidence::Ambiguous { .. }
    ));
}

#[test]
fn sign_agreement_must_be_a_strict_majority() {
    assert!(SignAgreement::new(0.5).is_none(), "половина — это ничья");
    assert!(SignAgreement::new(1.01).is_none());
    assert!(SignAgreement::new(f64::NAN).is_none());
    assert_eq!(SignAgreement::new(0.95).map(SignAgreement::fraction), Some(0.95));
}

/// Ядро отдало миллиамперы: 500 мА читаются как 500 «мкА» = 0,5 мА — ниже правдоподобного.
#[test]
fn plausible_current_range_flags_milliamps_reported_as_microamps() {
    let range = PlausibleCurrentRange::from_milliamps(10, 10_000).expect("диапазон");
    assert_eq!(
        range.classify(500_000.0),
        CurrentPlausibility::Plausible,
        "500 мА в мкА"
    );
    assert_eq!(range.classify(500.0), CurrentPlausibility::BelowRange, "500 мА в «мА»");
    assert_eq!(
        range.classify(10_000.0),
        CurrentPlausibility::Plausible,
        "граница включается"
    );
    assert_eq!(range.classify(10_000_001.0), CurrentPlausibility::AboveRange);
    assert_eq!(range.classify(f64::NAN), CurrentPlausibility::BelowRange);
    assert!(PlausibleCurrentRange::from_milliamps(0, 10).is_none());
    assert!(PlausibleCurrentRange::from_milliamps(10, 10).is_none());
    assert!(
        PlausibleCurrentRange::from_milliamps(1, u64::MAX).is_none(),
        "переполнение мкА"
    );
}
