use shell_contract::thermal::ThermalStatus;

use super::*;
use crate::power_measurement::AbPowerExperiment;
use crate::power_measurement::ab_experiment::{PairDelta, PairOrder, WindowReport};
use crate::power_measurement::{AbPowerReport, AbRejection, Contribution, WindowMeasurement};
use crate::scenario::camera::stage_budget_window::{RefreshReading, WindowRefusal};
use crate::scenario::camera::viewfinder_session::SessionSummary;
use crate::scenario::camera::viewfinder_stage_budget_report::{
    GpuWindowSample, RowPower, StageBudgetObservation, evaluate_stage_budget,
};

fn thermal(status: ThermalStatus, headroom: f32) -> ThermalSnapshot {
    ThermalSnapshot {
        status: Some(status),
        headroom: Some(headroom),
        error: None,
    }
}

fn measurement(ma: f64) -> WindowMeasurement {
    WindowMeasurement {
        mean_discharge_ma: ma,
        samples: 200,
        samples_without_current: 0,
        warmup_samples: 40,
        expected_samples: 200,
        missing_samples: 0,
        longest_gap_ms: 260,
        charge_used_mah: None,
        temperature_start: None,
        temperature_end: None,
    }
}

fn time(p50: f64) -> GpuTimeMs {
    GpuTimeMs {
        samples: 600,
        p50_ms: p50,
        p95_ms: p50 + 0.3,
        max_ms: p50 + 1.0,
    }
}

fn report(contribution: Contribution, on_ma: f64, off_ma: f64) -> AbPowerReport {
    AbPowerReport {
        contribution,
        discharge_sign: None,
        pairs: (0..3)
            .map(|pair| PairDelta {
                pair,
                order: PairOrder::AThenB,
                stage_on_ma: on_ma,
                stage_off_ma: off_ma,
                delta_ma: on_ma - off_ma,
            })
            .collect(),
        windows: (0..6)
            .map(|index| {
                let stage = AbPowerExperiment::stage_of(index);
                WindowReport {
                    index,
                    stage,
                    measurement: measurement(if stage == StageSetting::On { on_ma } else { off_ma }),
                }
            })
            .collect(),
        samples_before_start: 0,
        late_samples: 0,
    }
}

/// Окна строки: A с временем `on`, B с временем `off`; `None` — нет времени работы в окне.
fn windows(on: (Option<f64>, f64), off: (Option<f64>, f64), metric: CostMetricKind) -> Vec<GpuWindowSample> {
    (0..6)
        .map(|index| {
            let stage = AbPowerExperiment::stage_of(index);
            let (work_ms, frame_ms) = if stage == StageSetting::On { on } else { off };
            GpuWindowSample {
                index,
                stage,
                isp_dispatch: Some(time(2.0)),
                frame_total: Some(time(frame_ms)),
                setting_in_effect: true,
                ui_overlay: (metric == CostMetricKind::Ui).then(|| work_ms.map(time)).flatten(),
                scopes: (metric == CostMetricKind::Scopes).then(|| work_ms.map(time)).flatten(),
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CostMetricKind {
    Ui,
    Scopes,
}

fn condition(index: usize, refusal: Option<WindowRefusal>, hz: Option<f64>) -> WindowCondition {
    WindowCondition {
        index,
        stage: AbPowerExperiment::stage_of(index),
        thermal_start: thermal(ThermalStatus::Light, 0.6),
        thermal_end: thermal(ThermalStatus::Moderate, 0.9),
        display: hz.map(|mean_hz| RefreshReading {
            mean_hz,
            min_hz: mean_hz,
            max_hz: mean_hz,
            readings: 100,
        }),
        counters: None,
        refusal,
        issued: Default::default(),
    }
}

#[test]
fn ui_row_cost_is_the_ui_pass_time_minus_zero_because_it_did_not_run_in_window_b() {
    let row = StageBudgetRow::assemble(
        "ui_overlay",
        windows((Some(1.5), 3.0), (None, 2.0), CostMetricKind::Ui),
        RowPower::Ab {
            report: report(
                Contribution::Measured {
                    mean_delta_ma: 12.0,
                    spread_ma: 1.0,
                },
                520.0,
                508.0,
            ),
        },
    );
    assert_eq!(row.delta_gpu_p50_ms, Some(1.5), "1,5 − 0 (подавлено)");
    assert_eq!(row.delta_gpu_p95_ms, Some(1.8));
    assert_eq!(row.gpu_off.ui_overlay_p50_ms, None, "у подавленной работы времени нет");
    assert_eq!(row.table_line(), "ui_overlay | 1.50 / 1.80 | 12.0 ± 1.0 | measured");
}

#[test]
fn scopes_row_without_any_window_b_has_no_zero_to_invent() {
    let only_a: Vec<GpuWindowSample> = windows((Some(0.8), 3.0), (None, 2.0), CostMetricKind::Scopes)
        .into_iter()
        .filter(|window| window.stage == StageSetting::On)
        .collect();
    let row = StageBudgetRow::assemble(
        "color_scopes",
        only_a,
        RowPower::Ab {
            report: report(Contribution::Indistinguishable { spread_ma: 3.0 }, 520.0, 519.0),
        },
    );
    assert_eq!(row.delta_gpu_p50_ms, None, "окон B нет — ноль был бы выдумкой");
}

#[test]
fn whole_product_row_feeds_the_camera_without_product_gpu_baseline() {
    let row = StageBudgetRow::assemble(
        "whole_product",
        windows((Some(1.5), 3.5), (None, 0.7), CostMetricKind::Ui),
        RowPower::Ab {
            report: report(
                Contribution::Measured {
                    mean_delta_ma: 80.0,
                    spread_ma: 2.0,
                },
                580.0,
                500.0,
            ),
        },
    );
    assert_eq!(row.delta_gpu_p50_ms, Some(2.8), "кадр целиком: 3,5 − 0,7");
    let baseline = CameraWithoutProductGpuRow::from_rows(std::slice::from_ref(&row)).expect("строка есть");
    assert_eq!(baseline.frame_total_p50_ms, Some(0.7));
    assert_eq!(baseline.mean_discharge_ma, Some(500.0), "среднее окон B");
    assert_eq!(
        baseline.table_line(),
        "camera_without_product_gpu | 0.70 / 1.00 | 500.0 (абс.) | baseline"
    );
    assert!(CameraWithoutProductGpuRow::from_rows(&[]).is_none());
}

fn observation(rows: Vec<StageBudgetRow>, surface: SurfaceReport) -> StageBudgetObservation {
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
        requested_rows: rows.iter().map(|row| row.row).collect(),
        camera_without_isp: None,
        camera_without_product_gpu: CameraWithoutProductGpuRow::from_rows(&rows),
        rows,
        surface,
        gpu_window_frames: 600,
        undelivered: Vec::new(),
    }
}

#[test]
fn a_state_rejected_row_is_printed_as_rejected_and_the_violation_names_the_window_and_the_mismatch() {
    let refusal = WindowRefusal::StateNotApplied {
        mismatches: vec![crate::scenario::camera::stage_budget_window::StateMismatch::UiStillDrawing],
    };
    let row = StageBudgetRow::assemble(
        "ui_overlay",
        windows((Some(1.5), 3.0), (None, 2.0), CostMetricKind::Ui),
        RowPower::Ab {
            report: report(
                Contribution::Rejected {
                    reason: AbRejection::StateNotApplied {
                        index: 1,
                        stage: StageSetting::Off,
                    },
                },
                520.0,
                508.0,
            ),
        },
    )
    .with_conditions(vec![condition(0, None, None), condition(1, Some(refusal), None)]);

    assert_eq!(row.table_line(), "ui_overlay | — | — | rejected: state_not_applied");
    let violations = evaluate_stage_budget(&observation(
        vec![row],
        SurfaceReport::plain(StageBudgetSurface::PhotoScreen),
    ));
    let text = violations.join("; ");
    assert!(text.contains("окно 1 отвергнуто"), "{text}");
    assert!(text.contains("state_not_applied (ui_still_drawing)"), "{text}");
    assert!(!text.contains("A/B тока отказал"), "это не отказ тока: {text}");
}

#[test]
fn entry_and_exit_failures_are_violations_and_a_clean_surface_has_none() {
    let mut surface = SurfaceReport::plain(StageBudgetSurface::CalibrateScreen);
    assert!(surface.violations().is_empty());
    surface.entry_failure = Some("раздел калибровки не стал живым".to_owned());
    surface.exit_failure = Some("режим экрана Calibrate, нужен Photo".to_owned());
    let violations = surface.violations();
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(violations[0].contains("calibrate_screen") && violations[0].contains("не стал живым"));
    assert!(violations[1].contains("не вернулся в Photo"));
}

#[test]
fn summary_names_the_surface_the_scopes_request_and_compact_thermal_and_refresh_notes() {
    let row = StageBudgetRow::assemble(
        "display_120hz",
        windows((None, 3.0), (None, 3.0), CostMetricKind::Ui),
        RowPower::Ab {
            report: report(Contribution::Indistinguishable { spread_ma: 4.0 }, 520.0, 518.0),
        },
    )
    .with_conditions(
        (0..6)
            .map(|index| {
                condition(
                    index,
                    None,
                    Some(if AbPowerExperiment::stage_of(index) == StageSetting::On {
                        119.9
                    } else {
                        60.0
                    }),
                )
            })
            .collect(),
    );
    let mut surface = SurfaceReport::plain(StageBudgetSurface::CalibrateScreen);
    surface.scopes = Some(ScopesReport {
        request: Some(serde_json::json!({ "kind": "waveform", "image_size": { "width": 640, "height": 360 } })),
        float32_controls: serde_json::json!({ "rounding_mode_rte": true }),
        passes_recorded: 77,
        gpu_time: None,
    });
    let summary = observation(vec![row], surface).summary();

    assert_eq!(summary["surface"], "calibrate_screen");
    assert_eq!(summary["scopes"]["request"]["kind"], "waveform");
    assert_eq!(summary["scopes"]["float32_controls"]["rounding_mode_rte"], true);
    assert_eq!(summary["scopes"]["passes_at_entry"], 77);
    let notes = summary["row_notes"][0].as_str().expect("заметка");
    assert!(
        notes.starts_with("display_120hz: light 0.60 -> moderate 0.90"),
        "{notes}"
    );
    assert!(notes.contains("hz A 120 B 60"), "{notes}");
}

#[test]
fn the_longest_realistic_result_with_every_row_thermal_and_notes_still_fits_a_logcat_record() {
    use crate::logging::record::LOGCAT_MAX_TEXT_BYTES;
    use crate::result_line::RESULT_LINE_PREFIX;
    use crate::scenario::camera::result_budget::judge_within_budget;
    use crate::viewfinder_scenario_config::StageBudgetRowSetting;

    let rows: Vec<StageBudgetRow> = StageBudgetRowSetting::all_rows()
        .into_iter()
        .map(|setting| {
            StageBudgetRow::assemble(
                setting.name(),
                windows((Some(1.5), 3.0), (None, 2.0), CostMetricKind::Ui),
                RowPower::Ab {
                    report: report(
                        Contribution::Measured {
                            mean_delta_ma: 12.0,
                            spread_ma: 1.0,
                        },
                        520.0,
                        508.0,
                    ),
                },
            )
            .with_conditions((0..6).map(|index| condition(index, None, Some(120.0))).collect())
        })
        .collect();
    let mut surface = SurfaceReport::plain(StageBudgetSurface::CalibrateScreen);
    surface.scopes = Some(ScopesReport {
        request: Some(serde_json::json!({
            "kind": "rgb_parade", "vectorscope_axes": "oklab_ab", "scan": "upright",
            "image_size": { "width": 1024, "height": 512 }
        })),
        float32_controls: serde_json::json!({ "rounding_mode_rte": true, "denorm_preserve": true, "denorm_flush_to_zero": false }),
        passes_recorded: 1234,
        gpu_time: None,
    });
    let summary = observation(rows, surface).summary();
    let verdict = judge_within_budget(summary, Vec::new());
    let line = serde_json::json!({ "scenario": "viewfinder-stage-budget", "status": "passed", "report": match &verdict.outcome {
        crate::scenario::ScenarioOutcome::Passed { report } => report.clone(),
        other => panic!("ждали passed: {other:?}"),
    }});
    let bytes = RESULT_LINE_PREFIX.len() + line.to_string().len();
    assert!(bytes <= LOGCAT_MAX_TEXT_BYTES, "строка результата {bytes} байт");
}
