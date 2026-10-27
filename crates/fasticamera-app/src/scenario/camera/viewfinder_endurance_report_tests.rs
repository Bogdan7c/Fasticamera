use shell_contract::thermal::ThermalStatus;

use super::*;
use crate::power_measurement::endurance::{EnduranceSnapshot, EnduranceTotals, IspGpuTime, MinutePower};
use crate::power_measurement::{MinuteObservation, ThermalSnapshot, WindowRejection};
use crate::scenario::camera::viewfinder_metrics::IspCountersDelta;

fn thermal(status: ThermalStatus) -> ThermalSnapshot {
    ThermalSnapshot {
        status: Some(status),
        headroom: Some(0.6),
        error: None,
    }
}

/// Минута: тепло, частота, медиана ISP.
fn minute(minute: u32, status: ThermalStatus, fps: f64, isp_ms: f64) -> EnduranceSnapshot {
    EnduranceSnapshot {
        minute,
        power: MinutePower::Rejected {
            rejection: WindowRejection::NoCurrentReadings,
        },
        observation: MinuteObservation {
            thermal: thermal(status),
            sensor_fps: Some(fps),
            isp_gpu_time: Some(IspGpuTime {
                median_ms: isp_ms,
                p95_ms: isp_ms + 0.4,
            }),
        },
    }
}

fn report(minutes: Vec<EnduranceSnapshot>) -> EnduranceReport {
    EnduranceReport {
        discharge_sign: None,
        start: thermal(ThermalStatus::None),
        totals: EnduranceTotals {
            measured_minutes: 0,
            rejected_minutes: u32::try_from(minutes.len()).unwrap_or(0),
            mean_discharge_ma: None,
            charge_used_mah: None,
            battery_temperature_start: None,
            battery_temperature_end: None,
            worst_thermal_status: Some(ThermalStatus::None),
            peak_headroom: Some(0.6),
        },
        minutes,
        samples_before_start: 0,
    }
}

fn calm_ten_minutes() -> Vec<EnduranceSnapshot> {
    (1..=10)
        .map(|index| minute(index, ThermalStatus::None, 59.9, 2.0))
        .collect()
}

#[test]
fn calm_run_has_no_throttling_signs() {
    let config = ViewfinderEnduranceConfig::default();
    assert!(find_throttling(&config, &report(calm_ten_minutes())).is_empty());
}

#[test]
fn thermal_status_fps_drop_and_isp_growth_are_each_found_with_the_minute() {
    let config = ViewfinderEnduranceConfig::default();
    let mut minutes = calm_ten_minutes();
    minutes[6] = minute(7, ThermalStatus::Moderate, 59.9, 2.1);
    minutes[8] = minute(9, ThermalStatus::Light, 52.0, 2.2);
    minutes[9] = minute(10, ThermalStatus::Light, 59.0, 2.8);
    let signs = find_throttling(&config, &report(minutes));
    assert_eq!(
        signs,
        [
            ThrottlingSign::ThermalStatus {
                minute: 7,
                status: ThermalStatus::Moderate
            },
            ThrottlingSign::SensorFpsDrop { minute: 9, fps: 52.0 },
            ThrottlingSign::IspTimeGrowth {
                first_ms: 2.0,
                last_ms: 2.8,
                growth: 1.4
            },
        ]
    );
    // `light` ниже порога `moderate` — не признак.
}

#[test]
fn hot_start_is_reported_as_minute_zero() {
    let config = ViewfinderEnduranceConfig::default();
    let mut hot = report(calm_ten_minutes());
    hot.start = thermal(ThermalStatus::Severe);
    assert_eq!(
        find_throttling(&config, &hot),
        [ThrottlingSign::ThermalStatus {
            minute: 0,
            status: ThermalStatus::Severe
        }]
    );
}

fn observation(minutes: Vec<EnduranceSnapshot>) -> ViewfinderEnduranceObservation {
    let report = report(minutes);
    ViewfinderEnduranceObservation {
        failure: None,
        session: SessionSummary {
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            state_at_end: "streaming",
            events: Vec::new(),
        },
        expected_minutes: 10,
        power_unavailable: None,
        throttling: find_throttling(&ViewfinderEnduranceConfig::default(), &report),
        report,
        window: WindowDelta {
            window_ms: 600_000,
            isp: IspCountersDelta {
                frames_processed: 36_000,
                ..IspCountersDelta::default()
            },
            ..WindowDelta::default()
        },
    }
}

#[test]
fn throttling_is_a_finding_not_a_failure() {
    let mut minutes = calm_ten_minutes();
    minutes[4] = minute(5, ThermalStatus::Severe, 45.0, 2.0);
    let observation = observation(minutes);
    assert!(!observation.throttling.is_empty());
    assert!(evaluate_endurance(&observation).is_empty());
    let summary = observation.summary(2_000);
    assert_eq!(summary["throttling"]["found"], true);
    assert!(summary.to_string().len() < 2_900, "{} байт", summary.to_string().len());
}

#[test]
fn short_run_copies_and_isp_failures_fail() {
    let mut short = observation(calm_ten_minutes());
    short.report.minutes.truncate(7);
    assert!(evaluate_endurance(&short)[0].contains("7 из 10"));

    let mut copies = observation(calm_ten_minutes());
    copies.window.stream_cpu_copies = 1;
    assert!(evaluate_endurance(&copies)[0].contains("CPU-копий"));

    let mut broken = observation(calm_ten_minutes());
    broken.window.isp.params_failed = 2;
    assert!(evaluate_endurance(&broken)[0].contains("params_failed = 2"));

    let mut idle = observation(calm_ten_minutes());
    idle.window.isp.frames_processed = 0;
    assert!(evaluate_endurance(&idle)[0].contains("ни одного кадра"));
}

/// Худший провал: длинный прогон, в каждой минуте оба признака троттлинга и длинная ошибка
/// датчика тепла, источник тока недоступен с длинной причиной, все правила годности нарушены.
/// Строка результата обязана влезть в запись logcat и сохранить вывод о троттлинге.
#[test]
fn worst_case_failure_line_with_throttling_every_minute_fits_the_logcat_record() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };

    let minutes: Vec<EnduranceSnapshot> = (1..=60)
        .map(|index| {
            let mut snapshot = minute(index, ThermalStatus::Severe, 20.0, 2.0 + f64::from(index));
            snapshot.observation.thermal.error = Some(long_reason("тепло"));
            snapshot
        })
        .collect();
    let mut observation = observation(minutes);
    observation.session = worst_session();
    observation.expected_minutes = 61;
    observation.power_unavailable = Some(long_reason("ток"));
    observation.window.isp.frames_processed = 0;
    observation.window.isp.cpu_copies = 7;
    observation.window.isp.isp_record_failures = 7;
    observation.window.isp.compose_failures = 7;
    observation.window.isp.params_failed = 7;
    assert!(observation.throttling.len() > 100, "{}", observation.throttling.len());
    let mut violations = evaluate_endurance(&observation);
    assert!(violations.len() >= 5, "{violations:?}");
    observation.failure = Some(long_failure());
    violations.push(long_failure().describe());

    let verdict = judge_within_budget(
        observation.summary(super::super::viewfinder_endurance::ENDURANCE_SUMMARY_BUDGET_BYTES),
        violations.clone(),
    );
    let line = assert_failure_line_fits("viewfinder-endurance", verdict.outcome);
    let throttling = &line["report"]["throttling"];
    assert_eq!(throttling["found"], true);
    assert_eq!(throttling["sign_count"], observation.throttling.len());
    assert!(throttling["signs"].as_array().is_some_and(|signs| !signs.is_empty()));
    assert!(line["report"]["endurance"].get("totals").is_some(), "{line}");
    assert_eq!(verdict.violation_details.len(), violations.len());
}
