//! Окно замера: синтетические показания с известным током → известные числа.

use std::time::Duration;

use shell_contract::power::{BatteryCurrent, ChargingState, DeciCelsius, DischargeSign, PowerPlug};

use super::*;
use crate::power_measurement::test_support::{
    at_ms, charging, on_battery, period, phone_current_range, series, with_counter,
};

const ANDROID: DischargeSign = DischargeSign::NegativeWhenDischarging;

/// Окно 10 с с разогревом 2 с: измеряемая часть 8 с = 32 показания по 250 мс.
fn window() -> PowerWindowAccumulator {
    PowerWindowAccumulator::new(
        WindowSpan::new(at_ms(0), Duration::from_millis(2_000), Duration::from_millis(10_000)),
        period(),
    )
}

fn fill(accumulator: &mut PowerWindowAccumulator, samples: Vec<shell_contract::power::BatterySample>) {
    for sample in samples {
        accumulator.push(sample);
    }
}

#[test]
fn constant_current_gives_exact_mean_and_full_sample_accounting() {
    let mut accumulator = window();
    fill(&mut accumulator, series(0, 10_000, |_| 500));
    let measurement = accumulator
        .finish(ANDROID, 0.2, phone_current_range())
        .expect("годное окно");
    assert_eq!(measurement.mean_discharge_ma, 500.0);
    assert_eq!(measurement.warmup_samples, 8, "0..2000 мс по 250 мс");
    assert_eq!(measurement.samples, 32);
    assert_eq!(measurement.expected_samples, 32);
    assert_eq!(measurement.missing_samples, 0);
    assert_eq!(measurement.longest_gap_ms, 250);
    assert_eq!(measurement.charge_used_mah, None, "счётчика нет — не ноль, а None");
}

#[test]
fn warmup_samples_do_not_enter_the_mean() {
    let mut accumulator = window();
    // В разогреве ток 2000 мА (переходный процесс), дальше 400 мА.
    fill(
        &mut accumulator,
        series(0, 10_000, |ms| if ms < 2_000 { 2_000 } else { 400 }),
    );
    let measurement = accumulator
        .finish(ANDROID, 0.2, phone_current_range())
        .expect("годное окно");
    assert_eq!(measurement.mean_discharge_ma, 400.0);
}

#[test]
fn mean_of_varying_current_counter_delta_and_temperatures() {
    let mut accumulator = window();
    // В измеряемой части токи чередуются 300/700 мА → среднее 500; счётчик падает на 50 мкА·ч за
    // показание, температура растёт на 1 десятую за показание.
    let samples = series(2_000, 10_000, |ms| if (ms / 250) % 2 == 0 { 300 } else { 700 })
        .into_iter()
        .enumerate()
        .map(|(index, sample)| {
            let step = i64::try_from(index).expect("индекс");
            with_counter(
                sample,
                4_000_000 - 50 * step,
                300 + i32::try_from(index).expect("индекс"),
            )
        })
        .collect();
    fill(&mut accumulator, samples);
    let measurement = accumulator
        .finish(ANDROID, 0.2, phone_current_range())
        .expect("годное окно");
    assert_eq!(measurement.mean_discharge_ma, 500.0);
    assert_eq!(
        measurement.charge_used_mah,
        Some(1.55),
        "31 шаг × 50 мкА·ч = 1550 мкА·ч"
    );
    assert_eq!(measurement.temperature_start, Some(DeciCelsius::from_deci_celsius(300)));
    assert_eq!(measurement.temperature_end, Some(DeciCelsius::from_deci_celsius(331)));
}

#[test]
fn charging_sample_even_in_warmup_rejects_the_whole_window() {
    let mut accumulator = window();
    assert_eq!(accumulator.push(on_battery(0, 500)), WindowPush::WarmUp);
    let expected = WindowRejection::ExternalPower {
        plug: PowerPlug::Usb,
        charging: ChargingState::Charging,
    };
    assert_eq!(accumulator.push(charging(250, 500)), WindowPush::Rejected(expected));
    assert_eq!(
        accumulator.push(on_battery(5_000, 500)),
        WindowPush::Rejected(expected),
        "отказ липкий: годное показание после зарядки окно не спасает"
    );
    assert_eq!(accumulator.finish(ANDROID, 0.2, phone_current_range()), Err(expected));
}

#[test]
fn unknown_power_source_is_rejected_not_assumed_to_be_battery() {
    let mut accumulator = window();
    let mut sample = on_battery(3_000, 500);
    sample.plug = PowerPlug::Unknown;
    sample.charging = ChargingState::Unknown;
    assert_eq!(
        accumulator.push(sample),
        WindowPush::Rejected(WindowRejection::PowerStateUnknown {
            charging: ChargingState::Unknown
        })
    );
}

#[test]
fn missing_samples_are_counted_and_too_many_reject_the_window() {
    // 6 пропусков из 32 (0,1875) — окно годно, дыры посчитаны.
    let mut tolerable = window();
    fill(
        &mut tolerable,
        series(0, 10_000, |_| 500)
            .into_iter()
            .filter(|sample| !(5_000..6_500).contains(&(sample.at.as_nanos() / 1_000_000)))
            .collect(),
    );
    let measurement = tolerable
        .finish(ANDROID, 0.2, phone_current_range())
        .expect("годное окно");
    assert_eq!(measurement.samples, 26);
    assert_eq!(measurement.missing_samples, 6);
    assert_eq!(measurement.longest_gap_ms, 1_750, "от 4750 до 6500 мс");

    // 8 пропусков из 32 (0,25) — больше порога 0,2.
    let mut holey = window();
    fill(
        &mut holey,
        series(0, 10_000, |_| 500)
            .into_iter()
            .filter(|sample| !(5_000..7_000).contains(&(sample.at.as_nanos() / 1_000_000)))
            .collect(),
    );
    assert_eq!(
        holey.finish(ANDROID, 0.2, phone_current_range()),
        Err(WindowRejection::TooManyMissingSamples {
            missing: 8,
            expected: 32
        })
    );
}

#[test]
fn samples_without_current_are_counted_and_an_all_blank_window_is_rejected() {
    let mut partly_blank = window();
    let mut samples = series(2_000, 10_000, |_| 600);
    samples[0].current = None;
    samples[1].current = None;
    fill(&mut partly_blank, samples);
    let measurement = partly_blank
        .finish(ANDROID, 0.2, phone_current_range())
        .expect("годное окно");
    assert_eq!(measurement.samples_without_current, 2);
    assert_eq!(measurement.mean_discharge_ma, 600.0);

    let mut blank = window();
    fill(
        &mut blank,
        series(2_000, 10_000, |_| 600)
            .into_iter()
            .map(|mut sample| {
                sample.current = None;
                sample
            })
            .collect(),
    );
    assert_eq!(
        blank.finish(ANDROID, 0.2, phone_current_range()),
        Err(WindowRejection::NoCurrentReadings)
    );
}

#[test]
fn sign_convention_is_applied_not_guessed() {
    let mut accumulator = window();
    fill(
        &mut accumulator,
        series(2_000, 10_000, |_| 0)
            .into_iter()
            .map(|mut sample| {
                // Вендор с инвертированным знаком: разряд 450 мА — сырое +450 000 мкА.
                sample.current = Some(BatteryCurrent::from_raw_microamps(450_000));
                sample
            })
            .collect(),
    );
    assert_eq!(
        accumulator
            .finish(DischargeSign::PositiveWhenDischarging, 0.2, phone_current_range())
            .expect("окно")
            .mean_discharge_ma,
        450.0
    );
    assert_eq!(
        accumulator
            .finish(ANDROID, 0.2, phone_current_range())
            .expect("окно")
            .mean_discharge_ma,
        -450.0,
        "с другим соглашением тот же поток — заряд, а не разряд"
    );
}

#[test]
fn samples_outside_the_window_are_not_accepted() {
    let mut accumulator = window();
    assert_eq!(accumulator.push(on_battery(-1, 500)), WindowPush::OutsideWindow);
    assert_eq!(accumulator.push(on_battery(10_000, 500)), WindowPush::OutsideWindow);
    assert!(accumulator.measured_samples().is_empty());
}

/// Ядро отдаёт `CURRENT_NOW` в мА: 500 мА читаются как 500 «мкА». Окно не даёт число в 1000 раз
/// меньше настоящего, а отвергается как `UnitSuspect` — при любом знаке.
#[test]
fn milliamps_reported_as_microamps_reject_the_window_as_unit_suspect() {
    let mut accumulator = window();
    let milliamp_kernel: Vec<_> = series(0, 10_000, |_| 500)
        .into_iter()
        .map(|mut sample| {
            sample.current = Some(BatteryCurrent::from_raw_microamps(-500));
            sample
        })
        .collect();
    fill(&mut accumulator, milliamp_kernel);
    for sign in [ANDROID, DischargeSign::PositiveWhenDischarging] {
        assert_eq!(
            accumulator.finish(sign, 0.2, phone_current_range()),
            Err(WindowRejection::UnitSuspect {
                mean_abs_ma: 0.5,
                plausibility: CurrentPlausibility::BelowRange,
            })
        );
    }
}
