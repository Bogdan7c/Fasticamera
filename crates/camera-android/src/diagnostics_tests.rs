use camera_contract::fixtures::camera_id;

use super::*;
use crate::characteristics::CharacteristicIssue;

#[test]
fn fresh_diagnostics_are_all_zero_and_have_no_last_error() {
    let snapshot = BackendDiagnostics::new(4).snapshot();
    assert_eq!(snapshot.reader.frames_acquired, 0);
    assert_eq!(snapshot.reader.last_acquire_error, None);
    assert_eq!(snapshot.callback_panics, 0);
    assert!(snapshot.cameras.is_empty() && snapshot.recent_failures.is_empty());
}

#[test]
fn reader_counters_are_visible_through_the_backend_snapshot() {
    let diagnostics = BackendDiagnostics::new(4);
    let reader = diagnostics.reader();
    reader.record_frame_acquired();
    reader.record_frame_acquired();
    reader.record_pool_exhausted();
    reader.record_acquire_error(-10000);
    let snapshot = diagnostics.snapshot().reader;
    assert_eq!(snapshot.frames_acquired, 2);
    assert_eq!(snapshot.pool_exhausted, 1);
    assert_eq!(snapshot.last_acquire_error, Some(-10000));
    assert_eq!(snapshot.images_leaked, 0, "чужие счётчики не задеты");
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
    diagnostics.reader().record_frame_acquired();
    let json = serde_json::to_value(diagnostics.snapshot()).unwrap();
    assert_eq!(json["reader"]["frames_acquired"], 1);
    assert_eq!(json["reader"]["last_acquire_error"], serde_json::Value::Null);
}

#[test]
fn diagnostics_can_be_updated_from_other_threads_through_the_shared_handle() {
    let diagnostics = std::sync::Arc::new(BackendDiagnostics::new(4));
    let writers: Vec<_> = (0..4)
        .map(|_| {
            let shared = std::sync::Arc::clone(&diagnostics);
            std::thread::spawn(move || {
                for _ in 0..250 {
                    shared.reader().record_frame_acquired();
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(diagnostics.snapshot().reader.frames_acquired, 1000);
}

#[test]
fn origin_lookups_are_counted_by_reason_and_known_ones_are_free() {
    use camera_contract::three_a::RequestOrigin;
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_origin_lookup(OriginLookup::Known(RequestOrigin::Repeating));
    diagnostics.record_origin_lookup(OriginLookup::NoContext);
    diagnostics.record_origin_lookup(OriginLookup::NoContext);
    diagnostics.record_origin_lookup(OriginLookup::Unregistered);
    diagnostics.record_evicted_request_origin();
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.results_without_request_context, 2);
    assert_eq!(snapshot.results_with_unregistered_request, 1);
    assert_eq!(snapshot.evicted_request_origins, 1);
}

#[test]
fn companion_status_is_replaced_and_dead_three_a_is_visible_in_the_snapshot() {
    use crate::companion_stream::CompanionUnavailableReason;
    let diagnostics = BackendDiagnostics::new(4);
    assert_eq!(
        diagnostics.snapshot().three_a_companion,
        CompanionStreamStatus::NotConfigured
    );
    diagnostics.record_companion_status(CompanionStreamStatus::Active {
        width: 640,
        height: 480,
    });
    diagnostics.record_companion_status(CompanionStreamStatus::Unavailable {
        reason: CompanionUnavailableReason::RejectedByHal,
    });
    let status = diagnostics.snapshot().three_a_companion;
    assert!(status.hal_three_a_is_dead());
    assert_eq!(
        status,
        CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::RejectedByHal
        }
    );
}

#[test]
fn plan_notes_and_three_a_counters_accumulate_by_kind() {
    let diagnostics = BackendDiagnostics::new(4);
    diagnostics.record_plan_notes(&[]);
    diagnostics.record_plan_notes(&[PlanNote::AeRegionUnsupported, PlanNote::AeLockUnavailable]);
    diagnostics.record_plan_notes(&[PlanNote::AeRegionUnsupported]);
    diagnostics.record_unreadable_three_a_keys(2);
    diagnostics.record_companion_stray_images(3);
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.request_plan_notes["ae_region_unsupported"], 2);
    assert_eq!(snapshot.request_plan_notes["ae_lock_unavailable"], 1);
    assert_eq!(snapshot.request_plan_notes.len(), 2);
    assert_eq!(snapshot.unreadable_three_a_keys, 2);
    assert_eq!(snapshot.companion_stray_images, 3);
}
