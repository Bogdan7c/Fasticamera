use super::*;

fn measured(fps: f64) -> Measurement {
    Measurement {
        end_reason: EndReasonName::Completed,
        warmup_frames: 10,
        measured_frames: 100,
        drained_frames: 2,
        window_ms: 1_000.0,
        frames_per_second: Some(fps),
        frame_cost_ms: Some(1_000.0 / fps),
        passes: Vec::new(),
        unmeasured_pass_frames: 0,
        mismatched_pass_frames: 0,
        timeline_fps: vec![fps],
    }
}

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

fn quiet_render() -> RenderSilence {
    RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity::default(),
        delta: RenderActivity::default(),
    }
}

#[test]
fn measured_row_has_a_number_and_no_violation() {
    let ok = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: measured(250.0),
        },
    );
    assert!(ok.is_measured());
    assert_eq!(ok.violation(), None);
    assert_eq!(ok.verdict_word(), "ok");
}

#[test]
fn every_row_without_a_number_is_a_violation_with_its_own_reason() {
    let no_frames = row(
        "still",
        RowOutcome::NoFramesCompleted {
            measurement: Measurement {
                frames_per_second: None,
                frame_cost_ms: None,
                measured_frames: 0,
                ..measured(1.0)
            },
        },
    );
    assert_eq!(no_frames.verdict_word(), "no_frames");
    assert!(
        no_frames
            .violation()
            .expect("нарушение")
            .contains("не завершился ни один кадр")
    );

    let failed = row(
        "headroom_v4",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::BuildFailed, "не хватило памяти"),
        },
    );
    assert_eq!(failed.verdict_word(), "build_failed");
    let text = failed.violation().expect("нарушение");
    assert!(
        text.starts_with("headroom_v4:") && text.contains("не хватило памяти"),
        "{text}"
    );

    let skipped = row(
        "compositor",
        RowOutcome::NotRun {
            reason: NotRunReason::Cancelled,
        },
    );
    assert_eq!(skipped.verdict_word(), "not_run");
    assert!(skipped.violation().expect("нарушение").contains("не запускалась"));

    let cancelled = row(
        "viewfinder",
        RowOutcome::Measured {
            measurement: Measurement {
                end_reason: EndReasonName::Cancelled,
                ..measured(10.0)
            },
        },
    );
    assert_eq!(cancelled.verdict_word(), "cancelled");
    assert!(
        cancelled.violation().is_some(),
        "прерванная работа с неполным окном — не зачёт"
    );
}

#[test]
fn render_activity_delta_counts_only_product_gpu_work_not_presents() {
    let before = RenderActivity {
        frames_presented: 100,
        isp_frames: 10,
        recomposed_frames: 2,
        ui_frames_drawn: 5,
    };
    let after = RenderActivity {
        frames_presented: 700,
        isp_frames: 10,
        recomposed_frames: 3,
        ui_frames_drawn: 9,
    };
    let delta = after.since(&before);
    assert_eq!(delta.frames_presented, 600, "показы — факт, не нарушение");
    assert_eq!(delta.product_gpu_frames(), 5);
    // Счётчик не может «уйти назад»: вычитание насыщающее.
    assert_eq!(before.since(&after).product_gpu_frames(), 0);
}

#[test]
fn verdict_fails_on_missing_numbers_and_on_noisy_render_but_not_within_tolerance() {
    let summary = RunSummary {
        rows: vec![row(
            "viewfinder",
            RowOutcome::Measured {
                measurement: measured(300.0),
            },
        )],
        cancelled: false,
    };
    assert_eq!(violations(&summary, &quiet_render(), 0), Vec::<String>::new());

    let noisy = RenderSilence::Observed {
        before: RenderActivity::default(),
        after: RenderActivity {
            isp_frames: 4,
            ..RenderActivity::default()
        },
        delta: RenderActivity {
            isp_frames: 4,
            ..RenderActivity::default()
        },
    };
    assert_eq!(
        violations(&summary, &noisy, 4),
        Vec::<String>::new(),
        "ровно допуск — можно"
    );
    let found = violations(&summary, &noisy, 3);
    assert_eq!(found.len(), 1);
    assert!(
        found[0].contains("4 кадров продуктовой работы GPU") && found[0].contains("допуск 3"),
        "{found:?}"
    );

    let without_render = RenderSilence::RenderUnavailable {
        reason: "нет vsync".to_owned(),
    };
    assert_eq!(violations(&summary, &without_render, 0), Vec::<String>::new());

    let broken = RunSummary {
        rows: vec![row(
            "still",
            RowOutcome::NotRun {
                reason: NotRunReason::Cancelled,
            },
        )],
        cancelled: true,
    };
    assert_eq!(violations(&broken, &quiet_render(), 0).len(), 1);
}

#[test]
fn row_serializes_with_a_state_tag_for_the_detail_line() {
    let json = serde_json::to_value(row(
        "scopes_parade",
        RowOutcome::Failed {
            failure: WorkloadFailure::new(FailureKind::SlotsMismatch, "3 против 2"),
        },
    ))
    .expect("json");
    assert_eq!(json["outcome"]["state"], "failed");
    assert_eq!(json["outcome"]["failure"]["kind"], "slots_mismatch");
    assert_eq!(json["name"], "scopes_parade");
}
