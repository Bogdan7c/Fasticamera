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
        digest: None,
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
        cost: None,
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

fn digest_of(combined: &str) -> DigestOutcome {
    DigestOutcome::Computed {
        combined: combined.to_owned(),
        parts: Vec::new(),
        frame_note: None,
    }
}

fn header_for(requested: Vec<&'static str>) -> SummaryHeader {
    SummaryHeader {
        validation: ValidationState::Disabled,
        sensor: SensorSource::Fallback {
            reason: "тест".to_owned(),
        },
        look: LookSummary {
            preset: "As Eye".to_owned(),
            tone_base: look_contract::ToneBaseModel::Agx,
        },
        camera: CameraSilence::ClosedByScenario,
        requested,
        warmup_ms: 3_000,
        measure_ms: 10_000,
        frames_in_flight: 2,
        render_tolerance_frames: 5,
    }
}

fn quiet_silence() -> RenderSilence {
    RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity::default(),
        delta: RenderActivity::default(),
    }
}

#[test]
fn digest_row_shows_the_combined_digest_a_dash_when_unavailable_and_leaves_the_table_untouched() {
    let mut with_digest = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(200.0),
        },
    );
    with_digest.digest = Some(digest_of("0123456789abcdef"));
    assert_eq!(digest_row(&with_digest), "viewfinder|0123456789abcdef");
    assert_eq!(
        table_row(&with_digest),
        "viewfinder|200|5.00|ok",
        "формат таблицы не менялся"
    );

    let mut unavailable = with_digest.clone();
    unavailable.digest = Some(DigestOutcome::Unavailable {
        reason: "нет доступа".to_owned(),
    });
    assert_eq!(digest_row(&unavailable), "viewfinder|-");
    let mut absent = with_digest;
    absent.digest = None;
    assert_eq!(digest_row(&absent), "viewfinder|-");
}

#[test]
fn summary_carries_digests_for_the_baseline_but_moves_them_to_detail_when_the_line_would_overflow() {
    let names: Vec<&'static str> = crate::saturation_bench::catalog::baseline_names()
        .into_iter()
        .map(|name| &*Box::leak(name.into_boxed_str()))
        .collect();
    let rows_for = |names: &[&'static str]| -> Vec<WorkloadRow> {
        names
            .iter()
            .map(|name| {
                let mut one = row(
                    name,
                    RowOutcome::Measured {
                        measurement: measurement(100.0),
                    },
                );
                one.digest = Some(digest_of("0123456789abcdef"));
                one
            })
            .collect()
    };
    let baseline = RunSummary {
        rows: rows_for(&names),
        cancelled: false,
    };
    let summary = build_summary(&header_for(names.clone()), &baseline, &quiet_silence());
    assert_eq!(
        summary["digests"].as_array().map(Vec::len),
        Some(14),
        "14 работ: дайджесты в строке результата"
    );
    assert_eq!(summary["digests"][0], "viewfinder|0123456789abcdef");
    assert!(summary.get("digests_in_detail_only").is_none());
    assert!(serde_json::to_string(&summary).unwrap().len() <= SUMMARY_BUDGET_WITH_DIGESTS_BYTES);

    let all: Vec<&'static str> = crate::saturation_bench::catalog::entries()
        .map(|entry| entry.name)
        .collect();
    let everything = RunSummary {
        rows: rows_for(&all),
        cancelled: false,
    };
    let all_count = all.len();
    let big = build_summary(&header_for(all), &everything, &quiet_silence());
    assert!(
        big.get("digests").is_none(),
        "весь каталог не влезает: дайджестов в строке нет"
    );
    assert_eq!(big["digests_in_detail_only"], true);
    assert_eq!(
        big["table"].as_array().map(Vec::len),
        Some(all_count),
        "таблица на месте"
    );
}

#[test]
fn detail_item_carries_the_digest_parts_and_the_frame_note() {
    let mut one = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measurement(100.0),
        },
    );
    one.digest = Some(DigestOutcome::Computed {
        combined: "0123456789abcdef".to_owned(),
        parts: vec![crate::saturation_bench::digest_report::DigestPartRow {
            label: "image",
            digest: "00000000000000ab".to_owned(),
        }],
        frame_note: Some("last_frame=awb_statistics".to_owned()),
    });
    let item = detail_item(&one);
    assert_eq!(item["digest"]["state"], "computed");
    assert_eq!(item["digest"]["combined"], "0123456789abcdef");
    assert_eq!(item["digest"]["parts"][0]["label"], "image");
    assert_eq!(item["digest"]["frame_note"], "last_frame=awb_statistics");
}
