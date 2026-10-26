use camera_contract::fixtures::camera_id;

use super::*;
use crate::characteristics::CharacteristicIssue;

#[test]
fn fresh_diagnostics_are_all_zero_and_have_no_last_error() {
    let snapshot = BackendDiagnostics::new(4).snapshot();
    assert_eq!(
        snapshot.raw_sink,
        RawSinkSnapshot {
            frames_acquired: 0,
            frames_released: 0,
            acquire_errors: 0,
            last_acquire_error: None
        }
    );
    assert_eq!(snapshot.callback_panics, 0);
    assert!(snapshot.cameras.is_empty() && snapshot.recent_failures.is_empty());
}

#[test]
fn raw_sink_counters_count_each_event_and_keep_the_last_error_code() {
    let diagnostics = BackendDiagnostics::new(4);
    let sink = diagnostics.raw_sink();
    for _ in 0..3 {
        sink.record_frame_acquired();
    }
    sink.record_frame_released();
    sink.record_frame_released();
    sink.record_acquire_error(-30002);
    sink.record_acquire_error(-10000);
    assert_eq!(
        diagnostics.snapshot().raw_sink,
        RawSinkSnapshot {
            frames_acquired: 3,
            frames_released: 2,
            acquire_errors: 2,
            last_acquire_error: Some(-10000)
        }
    );
}

#[test]
fn event_counters_are_independent() {
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_callback_panic();
    diagnostics.record_callback_after_close();
    diagnostics.record_callback_after_close();
    diagnostics.record_unmatched_frame_number();
    diagnostics.record_retained_session_context();
    diagnostics.record_leaked_availability_context();
    let snapshot = diagnostics.snapshot();
    assert_eq!(
        (
            snapshot.callback_panics,
            snapshot.callbacks_after_close,
            snapshot.unmatched_frame_numbers,
            snapshot.retained_session_contexts,
            snapshot.leaked_availability_contexts
        ),
        (1, 2, 1, 1, 1)
    );
}

#[test]
fn failure_history_is_bounded_and_counts_what_it_dropped() {
    let diagnostics = BackendDiagnostics::new(2);
    diagnostics.record_failure("open", -10010, "IN_USE");
    diagnostics.record_failure("configure", -10009, "STREAM_CONFIGURE_FAIL");
    diagnostics.record_failure("request", -10002, "DISCONNECTED");
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.dropped_failures, 1);
    let operations: Vec<_> = snapshot
        .recent_failures
        .iter()
        .map(|failure| failure.operation.as_str())
        .collect();
    assert_eq!(operations, vec!["configure", "request"]);
}

#[test]
fn catalog_records_replace_older_ones_for_the_same_camera_and_reset_clears_them() {
    let diagnostics = BackendDiagnostics::new(4);
    let note = CharacteristicNote {
        tag: "ACAMERA_X",
        issue: CharacteristicIssue::Absent,
    };
    diagnostics.record_camera(&camera_id("2"), Vec::new(), vec![note.clone()]);
    diagnostics.record_camera(&camera_id("2"), Vec::new(), Vec::new());
    diagnostics.record_camera(&camera_id("3"), Vec::new(), vec![note]);
    diagnostics.record_skipped_camera("0", "нет ключа".to_owned());
    diagnostics.record_skipped_camera("0", "снова нет ключа".to_owned());
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.cameras.len(), 2);
    assert!(
        snapshot
            .cameras
            .iter()
            .find(|camera| camera.camera == "2")
            .unwrap()
            .notes
            .is_empty()
    );
    assert_eq!(
        snapshot.skipped_cameras,
        vec![SkippedCamera {
            camera: "0".to_owned(),
            reason: "снова нет ключа".to_owned()
        }]
    );
    diagnostics.reset_catalog();
    let cleared = diagnostics.snapshot();
    assert!(cleared.cameras.is_empty() && cleared.skipped_cameras.is_empty());
}

#[test]
fn snapshot_serializes_to_json_the_app_can_log() {
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.raw_sink().record_frame_acquired();
    let json = serde_json::to_value(diagnostics.snapshot()).unwrap();
    assert_eq!(json["raw_sink"]["frames_acquired"], 1);
    assert_eq!(json["raw_sink"]["last_acquire_error"], serde_json::Value::Null);
}

#[test]
fn diagnostics_can_be_updated_from_other_threads_through_the_shared_handle() {
    let diagnostics = std::sync::Arc::new(BackendDiagnostics::new(4));
    let writers: Vec<_> = (0..4)
        .map(|_| {
            let shared = std::sync::Arc::clone(&diagnostics);
            std::thread::spawn(move || {
                for _ in 0..250 {
                    shared.raw_sink().record_frame_acquired();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(diagnostics.snapshot().raw_sink.frames_acquired, 1000);
}
