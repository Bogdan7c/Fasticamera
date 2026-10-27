//! Длинный прогон: снимки по минутам, итоги, отказ минуты на зарядке, сводка в бюджете.

use std::cell::Cell;
use std::time::Duration;

use shell_contract::thermal::ThermalStatus;

use super::*;
use crate::power_measurement::config::EnduranceSettings;
use crate::power_measurement::test_support::{at_ms, charging, series, settings, with_counter};

/// «Минута» в тестах — 10 с, прогон — 10 «минут».
const MINUTE_MS: i64 = 10_000;
/// Бюджет сводки в строке результата у сценариев потока (как `raw_stream`).
const SUMMARY_BUDGET_BYTES: usize = 2_900;

fn test_settings() -> PowerSettings {
    PowerSettings {
        endurance: EnduranceSettings {
            duration: Duration::from_millis(10 * 10_000),
            snapshot_interval: Duration::from_millis(10_000),
        },
        ..settings(10_000, 2_000, 3)
    }
}

fn thermal(status: ThermalStatus, headroom: f32) -> ThermalSnapshot {
    ThermalSnapshot {
        status: Some(status),
        headroom: Some(headroom),
        error: None,
    }
}

/// Показания всего прогона: ток 500 мА, счётчик −10 мкА·ч за показание, температура +1 десятая
/// за минуту.
fn run_samples() -> Vec<BatterySample> {
    series(0, 10 * MINUTE_MS, |_| 500)
        .into_iter()
        .enumerate()
        .map(|(index, sample)| {
            let step = i64::try_from(index).expect("индекс");
            let minute = i32::try_from(index / 40).expect("минута");
            with_counter(sample, 5_000_000 - 10 * step, 300 + minute)
        })
        .collect()
}

#[test]
fn ten_minutes_give_ten_snapshots_with_exact_totals_and_thermal_extremes() {
    let mut recorder = EnduranceRecorder::start(&test_settings(), at_ms(0), thermal(ThermalStatus::None, 0.5));
    assert_eq!(recorder.total_minutes(), 10);
    let observed = Cell::new(0_u32);
    let observe = || {
        observed.set(observed.get() + 1);
        // Тепло растёт к концу: статус Light с 6-й минуты, headroom 0,6 → 0,69.
        let minute = observed.get();
        let status = if minute >= 6 {
            ThermalStatus::Light
        } else {
            ThermalStatus::None
        };
        MinuteObservation {
            thermal: thermal(status, 0.59 + 0.01 * minute as f32),
            sensor_fps: None,
            isp_gpu_time: None,
        }
    };
    let step = recorder.on_tick(at_ms(10 * MINUTE_MS), &run_samples(), observe);
    assert_eq!(step, EnduranceStep::Finished);
    assert_eq!(observed.get(), 10, "наблюдение — ровно раз на минуту");

    let report = recorder.report();
    assert_eq!(report.minutes.len(), 10);
    assert_eq!(
        report.minutes.iter().map(|minute| minute.minute).collect::<Vec<_>>(),
        (1..=10).collect::<Vec<_>>()
    );
    let first = match report.minutes[0].power {
        MinutePower::Measured(measurement) => measurement,
        other => panic!("минута 1: {other:?}"),
    };
    assert_eq!(first.samples, 40);
    assert_eq!(first.charge_used_mah, Some(0.39), "39 шагов × 10 мкА·ч");

    let totals = report.totals;
    assert_eq!(totals.measured_minutes, 10);
    assert_eq!(totals.rejected_minutes, 0);
    assert_eq!(totals.mean_discharge_ma, Some(500.0));
    let charge = totals.charge_used_mah.expect("счётчик есть");
    assert!((charge - 3.9).abs() < 1e-9, "{charge}");
    assert_eq!(
        totals.battery_temperature_start,
        Some(DeciCelsius::from_deci_celsius(300))
    );
    assert_eq!(
        totals.battery_temperature_end,
        Some(DeciCelsius::from_deci_celsius(309))
    );
    assert_eq!(totals.worst_thermal_status, Some(ThermalStatus::Light));
    assert_eq!(totals.peak_headroom, Some(0.59 + 0.01 * 10.0));
    assert_eq!(report.discharge_sign, Some(DischargeSign::NegativeWhenDischarging));
}

#[test]
fn minute_on_external_power_is_rejected_but_the_run_continues() {
    let mut recorder = EnduranceRecorder::start(&test_settings(), at_ms(0), thermal(ThermalStatus::None, 0.5));
    let mut samples = run_samples();
    // Минута 4 (30–40 с): кабель подключили на 35 с.
    let plugged = samples
        .iter()
        .position(|sample| sample.at == at_ms(35_000))
        .expect("показание");
    samples[plugged] = charging(35_000, 500);
    let observe = || MinuteObservation {
        thermal: thermal(ThermalStatus::None, 0.5),
        sensor_fps: Some(60.0),
        isp_gpu_time: None,
    };
    assert_eq!(
        recorder.on_tick(at_ms(100_000), &samples, observe),
        EnduranceStep::Finished
    );
    let report = recorder.report();
    assert!(matches!(
        report.minutes[3].power,
        MinutePower::Rejected {
            rejection: WindowRejection::ExternalPower { .. }
        }
    ));
    assert_eq!(report.totals.measured_minutes, 9);
    assert_eq!(report.totals.rejected_minutes, 1);
    assert_eq!(report.totals.mean_discharge_ma, Some(500.0));
    assert_eq!(
        report.minutes[3].observation.sensor_fps,
        Some(60.0),
        "тепло и частота записаны"
    );
}

#[test]
fn ticks_close_minutes_as_time_passes() {
    let mut recorder = EnduranceRecorder::start(&test_settings(), at_ms(0), thermal(ThermalStatus::None, 0.5));
    let samples = run_samples();
    let observe = || MinuteObservation {
        thermal: thermal(ThermalStatus::None, 0.5),
        sensor_fps: None,
        isp_gpu_time: None,
    };
    let early: Vec<_> = samples
        .iter()
        .copied()
        .filter(|sample| sample.at < at_ms(15_000))
        .collect();
    assert_eq!(
        recorder.on_tick(at_ms(15_000), &early, observe),
        EnduranceStep::Recording { minute: 2 }
    );
    assert_eq!(recorder.report().minutes.len(), 1);
    // Тишина в кольце — минуты всё равно закрываются по часам (с отказом из-за дыр).
    assert_eq!(
        recorder.on_tick(at_ms(30_000), &[], observe),
        EnduranceStep::Recording { minute: 4 }
    );
    let report = recorder.report();
    assert_eq!(report.minutes.len(), 3);
    assert!(matches!(
        report.minutes[2].power,
        MinutePower::Rejected {
            rejection: WindowRejection::TooManyMissingSamples {
                missing: 40,
                expected: 40
            }
        }
    ));
}

#[test]
fn ambiguous_sign_marks_the_minute_and_a_later_clean_minute_establishes_it() {
    let mut recorder = EnduranceRecorder::start(&test_settings(), at_ms(0), thermal(ThermalStatus::None, 0.5));
    let mut samples = series(0, MINUTE_MS, |ms| if (ms / 250) % 2 == 0 { 500 } else { -500 });
    samples.extend(series(MINUTE_MS, 2 * MINUTE_MS, |_| 450));
    let observe = || MinuteObservation {
        thermal: thermal(ThermalStatus::None, 0.5),
        sensor_fps: None,
        isp_gpu_time: None,
    };
    recorder.on_tick(at_ms(2 * MINUTE_MS), &samples, observe);
    let report = recorder.report();
    assert!(matches!(
        report.minutes[0].power,
        MinutePower::SignUndetermined {
            evidence: DischargeSignEvidence::Ambiguous { .. }
        }
    ));
    assert!(
        matches!(report.minutes[1].power, MinutePower::Measured(measurement) if measurement.mean_discharge_ma == 450.0)
    );
    assert_eq!(
        report.totals.mean_discharge_ma,
        Some(450.0),
        "неопределённая минута в итог не вошла"
    );
}

#[test]
fn summary_fits_the_result_line_budget_and_minutes_go_to_detail_lines() {
    let mut recorder = EnduranceRecorder::start(&test_settings(), at_ms(0), thermal(ThermalStatus::None, 0.5));
    let observe = || MinuteObservation {
        thermal: thermal(ThermalStatus::None, 0.5),
        sensor_fps: Some(59.94),
        isp_gpu_time: Some(IspGpuTime {
            median_ms: 2.5,
            p95_ms: 3.1,
        }),
    };
    recorder.on_tick(at_ms(100_000), &run_samples(), observe);
    let report = recorder.report();

    let summary = report.summary_within(SUMMARY_BUDGET_BYTES);
    let summary_json = serde_json::to_string(&summary).expect("json");
    assert!(
        summary_json.len() <= SUMMARY_BUDGET_BYTES,
        "{} байт",
        summary_json.len()
    );
    assert_eq!(
        summary["minutes_in_detail"], true,
        "10 полных минут в 2900 байт не влезают"
    );
    assert_eq!(summary["totals"]["measured_minutes"], 10);
    assert_eq!(summary["last_minute"]["minute"], 10);

    let generous = report.summary_within(usize::MAX);
    assert_eq!(
        generous["minutes"].as_array().map(Vec::len),
        Some(10),
        "влезает — целиком"
    );

    let details = report.detail_items();
    assert_eq!(details.len(), 10);
    assert_eq!(details[4]["minute"], 5);
    assert_eq!(details[4]["observation"]["isp_gpu_time"]["median_ms"], 2.5);
    assert_eq!(details[4]["power"]["power"], "measured");
}
