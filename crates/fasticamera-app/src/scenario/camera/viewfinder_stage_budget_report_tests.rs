use shell_contract::power::{ChargingState, PowerPlug};

use super::*;
use crate::power_measurement::ab_experiment::{PairDelta, PairOrder, WindowReport};
use crate::power_measurement::{AbRejection, WindowMeasurement, WindowRejection};

fn measurement(mean_discharge_ma: f64) -> WindowMeasurement {
    WindowMeasurement {
        mean_discharge_ma,
        samples: 200,
        samples_without_current: 0,
        warmup_samples: 40,
        expected_samples: 200,
        missing_samples: 0,
        longest_gap_ms: 260,
        charge_used_mah: Some(8.0),
        temperature_start: None,
        temperature_end: None,
    }
}

/// A/B: окна A по `on_ma`, B по `off_ma`.
fn ab_report(on_ma: f64, off_ma: f64, contribution: Contribution) -> AbPowerReport {
    let windows = (0..6)
        .map(|index| {
            let stage = crate::power_measurement::AbPowerExperiment::stage_of(index);
            WindowReport {
                index,
                stage,
                measurement: measurement(if stage == StageSetting::On { on_ma } else { off_ma }),
            }
        })
        .collect();
    AbPowerReport {
        contribution,
        discharge_sign: None,
        pairs: (0..3)
            .map(|pair| PairDelta {
                pair,
                order: if pair % 2 == 0 {
                    PairOrder::AThenB
                } else {
                    PairOrder::BThenA
                },
                stage_on_ma: on_ma,
                stage_off_ma: off_ma,
                delta_ma: on_ma - off_ma,
            })
            .collect(),
        windows,
        samples_before_start: 0,
        late_samples: 0,
    }
}

fn gpu(index: usize, isp_p50: Option<f64>, total_p50: f64) -> GpuWindowSample {
    let time = |p50: f64| GpuTimeMs {
        samples: 600,
        p50_ms: p50,
        p95_ms: p50 + 0.3,
        max_ms: p50 + 1.0,
    };
    GpuWindowSample {
        index,
        stage: crate::power_measurement::AbPowerExperiment::stage_of(index),
        isp_dispatch: isp_p50.map(time),
        frame_total: Some(time(total_p50)),
        setting_in_effect: true,
    }
}

/// Окна стадии: со стадией ISP 2,0 мс, без — 1,6 мс.
fn stage_windows() -> Vec<GpuWindowSample> {
    (0..6)
        .map(
            |index| match crate::power_measurement::AbPowerExperiment::stage_of(index) {
                StageSetting::On => gpu(index, Some(2.0), 3.0),
                StageSetting::Off => gpu(index, Some(1.6), 2.6),
            },
        )
        .collect()
}

/// Окна всего видоискателя: ISP (кадр 3,0 мс) против мозаики (кадр 0,8 мс, прохода ISP нет).
fn whole_windows() -> Vec<GpuWindowSample> {
    (0..6)
        .map(
            |index| match crate::power_measurement::AbPowerExperiment::stage_of(index) {
                StageSetting::On => gpu(index, Some(2.0), 3.0),
                StageSetting::Off => gpu(index, None, 0.8),
            },
        )
        .collect()
}

fn measured(delta: f64) -> Contribution {
    Contribution::Measured {
        mean_delta_ma: delta,
        spread_ma: 2.0,
    }
}

#[test]
fn stage_row_cost_is_isp_pass_difference_and_table_line_has_four_columns() {
    let row = StageBudgetRow::assemble(
        "tone",
        stage_windows(),
        RowPower::Ab {
            report: ab_report(520.0, 510.0, measured(10.0)),
        },
    );
    assert_eq!(row.gpu_on.windows, 3);
    assert_eq!(row.gpu_on.isp_dispatch_p50_ms, Some(2.0));
    assert_eq!(row.gpu_off.isp_dispatch_p50_ms, Some(1.6));
    assert_eq!(row.delta_gpu_p50_ms, Some(0.4));
    assert_eq!(row.delta_gpu_p95_ms, Some(0.4));
    assert_eq!(row.table_line(), "tone | 0.40 / 0.40 | 10.0 ± 2.0 | measured");
}

#[test]
fn whole_viewfinder_compares_frame_total_and_feeds_the_camera_without_isp_row() {
    let whole = StageBudgetRow::assemble(
        WHOLE_VIEWFINDER_ROW,
        whole_windows(),
        RowPower::Ab {
            report: ab_report(560.0, 480.0, measured(80.0)),
        },
    );
    assert_eq!(
        whole.delta_gpu_p50_ms,
        Some(2.2),
        "у мозаики нет прохода ISP — сравнение по кадру"
    );
    let baseline = CameraWithoutIspRow::from_rows(std::slice::from_ref(&whole)).expect("строка есть");
    assert_eq!(baseline.frame_total_p50_ms, Some(0.8));
    assert_eq!(baseline.mean_discharge_ma, Some(480.0), "среднее окон B");
    assert_eq!(
        baseline.table_line(),
        "camera_without_isp | 0.80 / 1.10 | 480.0 (абс.) | baseline"
    );
    assert!(
        CameraWithoutIspRow::from_rows(&[]).is_none(),
        "без строки «весь видоискатель» базы нет"
    );
}

#[test]
fn indistinguishable_and_rejected_rows_are_written_as_such() {
    let quiet = StageBudgetRow::assemble(
        "oetf",
        stage_windows(),
        RowPower::Ab {
            report: ab_report(510.0, 509.0, Contribution::Indistinguishable { spread_ma: 6.0 }),
        },
    );
    assert!(
        quiet
            .table_line()
            .ends_with("не различим (разброс 6.0) | indistinguishable"),
        "{}",
        quiet.table_line()
    );
    let charging = Contribution::Rejected {
        reason: AbRejection::Window {
            index: 0,
            stage: StageSetting::On,
            rejection: WindowRejection::ExternalPower {
                plug: PowerPlug::Usb,
                charging: ChargingState::Charging,
            },
        },
    };
    let rejected = StageBudgetRow::assemble(
        "levels",
        Vec::new(),
        RowPower::Ab {
            report: ab_report(0.0, 0.0, charging),
        },
    );
    assert_eq!(
        rejected.table_line(),
        "levels | нет замеров | — | rejected: external_power"
    );
}

fn observation(rows: Vec<StageBudgetRow>, requested: Vec<&'static str>) -> StageBudgetObservation {
    StageBudgetObservation {
        failure: None,
        power_refusal: None,
        session: SessionSummary {
            configured: None,
            degraded: None,
            time_to_streaming_ms: None,
            time_to_live_ms: None,
            state_at_end: "streaming",
            events: Vec::new(),
        },
        requested_rows: requested,
        camera_without_isp: CameraWithoutIspRow::from_rows(&rows),
        rows,
        gpu_window_frames: 600,
        undelivered: Vec::new(),
    }
}

fn good_row(name: &'static str) -> StageBudgetRow {
    StageBudgetRow::assemble(
        name,
        stage_windows(),
        RowPower::Ab {
            report: ab_report(520.0, 510.0, measured(10.0)),
        },
    )
}

#[test]
fn complete_table_passes_and_summary_carries_the_table() {
    let observation = observation(vec![good_row("levels"), good_row("tone")], vec!["levels", "tone"]);
    assert!(evaluate_stage_budget(&observation).is_empty());
    let summary = observation.summary();
    assert_eq!(summary["table"][1], "tone | 0.40 / 0.40 | 10.0 ± 2.0 | measured");
    assert!(summary.to_string().len() < 2_900);
}

#[test]
fn power_refusal_window_not_in_effect_missing_gpu_and_undelivered_fail() {
    let mut refused = observation(Vec::new(), vec!["levels"]);
    refused.power_refusal = Some("опрос батареи не запущен".to_owned());
    let violations = evaluate_stage_budget(&refused);
    assert_eq!(
        violations.len(),
        1,
        "незаконченные строки при отказе тока не дублируются: {violations:?}"
    );
    assert!(violations[0].contains("ток не измерен"));

    let mut stray = good_row("tone");
    stray.windows[2].setting_in_effect = false;
    let violations = evaluate_stage_budget(&observation(vec![stray], vec!["tone"]));
    assert!(
        violations.iter().any(|violation| violation.contains("в 1 окнах")),
        "{violations:?}"
    );

    let no_gpu = StageBudgetRow::assemble(
        "tone",
        Vec::new(),
        RowPower::Ab {
            report: ab_report(520.0, 510.0, measured(10.0)),
        },
    );
    assert!(evaluate_stage_budget(&observation(vec![no_gpu], vec!["tone"]))[0].contains("нет времени GPU"));

    let missing_rows = observation(vec![good_row("tone")], vec!["tone", "oetf"]);
    assert!(evaluate_stage_budget(&missing_rows)[0].contains("1 из 2"));

    let mut undelivered = observation(vec![good_row("tone")], vec!["tone"]);
    undelivered.undelivered.push("RenderGone".to_owned());
    assert!(evaluate_stage_budget(&undelivered)[0].contains("не доставлена"));
}

#[test]
fn detail_items_each_fit_a_detail_line() {
    let observation = observation(vec![good_row("levels")], vec!["levels"]);
    for item in observation.detail_items() {
        let bytes = item.to_string().len();
        assert!(
            bytes < super::super::detail::DETAIL_LINE_BUDGET_BYTES,
            "элемент {bytes} байт: {item}"
        );
    }
}

/// Худший провал: все строки таблицы (8 стадий + весь видоискатель), в каждой — окна без
/// настройки, нет опроса батареи с длинной причиной; отказ тока и недоставленные регулировки тоже
/// с длинными причинами. Строка результата обязана влезть в запись logcat и сохранить таблицу.
#[test]
fn worst_case_failure_line_with_every_row_fits_the_logcat_record() {
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::scenario::camera::result_budget::worst_case::{
        assert_failure_line_fits, long_failure, long_reason, worst_session,
    };
    use crate::viewfinder_scenario_config::StageBudgetRowSetting;

    let names: Vec<&'static str> = StageBudgetRowSetting::default_rows()
        .into_iter()
        .map(StageBudgetRowSetting::name)
        .collect();
    let rows: Vec<StageBudgetRow> = names
        .iter()
        .map(|name| {
            let mut windows = if *name == WHOLE_VIEWFINDER_ROW {
                whole_windows()
            } else {
                stage_windows()
            };
            windows.iter_mut().for_each(|window| window.setting_in_effect = false);
            StageBudgetRow::assemble(
                name,
                windows,
                RowPower::SamplingUnavailable {
                    reason: long_reason("батарея"),
                },
            )
        })
        .collect();
    let mut observation = observation(rows, names.clone());
    observation.session = worst_session();
    observation.power_refusal = Some(long_reason("ток"));
    observation.undelivered = (0..40)
        .map(|index| long_reason(&format!("регулировка {index}")))
        .collect();
    let mut violations = evaluate_stage_budget(&observation);
    assert!(violations.len() > 2 * names.len(), "{}", violations.len());
    observation.failure = Some(long_failure());
    violations.push(long_failure().describe());

    let verdict = judge_within_budget(observation.summary(), violations.clone());
    let line = assert_failure_line_fits("viewfinder-stage-budget", verdict.outcome);
    assert_eq!(
        line["report"]["table"].as_array().map(Vec::len),
        Some(names.len() + 1),
        "таблица целиком (+ строка «камера без ISP»)"
    );
    assert_eq!(line["report"]["undelivered"]["count"], 40);
    assert_eq!(verdict.violation_details.len(), violations.len());
}
