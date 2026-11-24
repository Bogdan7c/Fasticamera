use super::*;
use crate::power_measurement::ThermalSnapshot;
use crate::saturation_bench::report::{
    EndReasonName, FailureKind, Measurement, NotRunReason, RenderActivity, RowOutcome, WorkloadFailure,
};

fn row(name: &'static str, outcome: RowOutcome) -> WorkloadRow {
    WorkloadRow {
        name,
        index: 1,
        total: 1,
        outcome,
        scene: None,
        ui: None,
        thermal_start: None,
        thermal_end: None,
        started_at_ms: None,
    }
}

fn measurement(fps: f64) -> Measurement {
    Measurement {
        end_reason: EndReasonName::Completed,
        warmup_frames: 1,
        measured_frames: 10,
        drained_frames: 0,
        window_ms: 1_000.0,
        frames_per_second: Some(fps),
        frame_cost_ms: Some(1_000.0 / fps),
        passes: Vec::new(),
        unmeasured_pass_frames: 0,
        mismatched_pass_frames: 0,
        timeline_fps: vec![fps],
    }
}

#[test]
fn three_significant_digits_at_every_magnitude() {
    assert_eq!(format_sig3(397.4), "397");
    assert_eq!(format_sig3(3837.2), "3837", "целая часть не режется");
    assert_eq!(format_sig3(45.67), "45.7");
    assert_eq!(format_sig3(7.891), "7.89");
    assert_eq!(format_sig3(0.2614), "0.261");
    assert_eq!(format_sig3(0.0), "0");
}

#[test]
fn table_row_is_short_and_carries_name_fps_cost_and_verdict() {
    let measured = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(200.0),
        },
    );
    assert_eq!(table_row(&measured), "viewfinder|200|5.00|ok");
    let failed = row(
        "still",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::BuildFailed, "очень длинная причина ".repeat(100)),
        },
    );
    assert_eq!(
        table_row(&failed),
        "still|-|-|build_failed",
        "причина — в подробностях, не в таблице"
    );
    let skipped = row(
        "compositor",
        RowOutcome::NotRun {
            reason: NotRunReason::Cancelled,
        },
    );
    assert_eq!(table_row(&skipped), "compositor|-|-|not_run");
}

#[test]
fn summary_names_the_sensor_source_the_look_and_what_render_did() {
    let header = SummaryHeader {
        validation: ValidationState::Disabled,
        sensor: SensorSource::Fallback {
            reason: "движок камеры недоступен".to_owned(),
        },
        look: LookSummary {
            preset: "As Eye".to_owned(),
            tone_base: look_contract::ToneBaseModel::Agx,
        },
        camera: CameraSilence::ClosedByScenario,
        requested: vec!["viewfinder"],
        warmup_ms: 3_000,
        measure_ms: 10_000,
        frames_in_flight: 2,
        render_tolerance_frames: 5,
    };
    let run = RunSummary {
        rows: vec![row(
            "viewfinder",
            RowOutcome::Measured {
                measurement: measurement(100.0),
            },
        )],
        cancelled: false,
    };
    let silence = RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity {
            frames_presented: 50,
            ..RenderActivity::default()
        },
        delta: RenderActivity {
            frames_presented: 50,
            ..RenderActivity::default()
        },
    };
    let summary = build_summary(&header, &run, &silence);
    assert_eq!(summary["table"][0], "viewfinder|100|10.0|ok");
    assert_eq!(summary["sensor"]["source"], "fallback");
    assert_eq!(summary["look"]["preset"], "As Eye");
    assert_eq!(summary["look"]["tone_base"], "agx");
    assert_eq!(summary["camera"], "closed_by_scenario");
    assert_eq!(summary["validation"], "disabled");
    assert_eq!(
        summary["render"]["delta"]["frames_presented"], 50,
        "показы рендера — факт в результате"
    );
    assert_eq!(summary["measured"], 1);
    assert_eq!(summary["requested"], serde_json::json!(["viewfinder"]));
    assert_eq!(summary["render_tolerance_frames"], 5);
}

#[test]
fn detail_item_keeps_the_typed_failure_and_thermal_words() {
    let mut failed = row(
        "still",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::ObjectsNotReleased, "Image 3->5"),
        },
    );
    failed.thermal_start = Some(ThermalSnapshot {
        status: Some(shell_contract::thermal::ThermalStatus::Light),
        headroom: Some(0.5),
        error: None,
    });
    failed.thermal_end = Some(ThermalSnapshot {
        status: None,
        headroom: None,
        error: Some("нет".to_owned()),
    });
    let item = detail_item(&failed);
    assert_eq!(item["outcome"]["failure"]["kind"], "objects_not_released");
    assert!(
        item["outcome"]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("Image 3->5")
    );
    assert_eq!(item["thermal_words"]["start"], "light");
    assert_eq!(item["thermal_words"]["end"], "unknown");
}
