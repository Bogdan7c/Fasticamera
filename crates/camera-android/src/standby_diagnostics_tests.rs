use super::*;

const SLOT0: StandbySlotId = StandbySlotId::new(0);
const SLOT1: StandbySlotId = StandbySlotId::new(1);

#[test]
fn fresh_diagnostics_report_no_slots() {
    assert!(StandbyDiagnostics::default().snapshot().is_empty());
}

#[test]
fn counters_are_kept_per_slot_and_independent() {
    let diagnostics = StandbyDiagnostics::default();
    diagnostics.record_session_started(SLOT0);
    diagnostics.record_result_offered(SLOT0);
    diagnostics.record_result_offered(SLOT0);
    diagnostics.record_capture_failure(SLOT1);
    diagnostics.record_swap(SLOT0);
    diagnostics.record_swap_refusal(SLOT1);
    diagnostics.record_open_failure(SLOT1);
    diagnostics.record_configure_failure(SLOT1);
    let snapshot = diagnostics.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(
        snapshot[0],
        StandbySlotSnapshot {
            slot: 0,
            sessions_started: 1,
            results_offered: 2,
            swaps: 1,
            ..StandbySlotSnapshot::empty(0)
        }
    );
    assert_eq!(
        snapshot[1],
        StandbySlotSnapshot {
            slot: 1,
            capture_failures: 1,
            swap_refusals: 1,
            open_failures: 1,
            configure_failures: 1,
            ..StandbySlotSnapshot::empty(1)
        }
    );
}

#[test]
fn slot_beyond_the_table_is_ignored_without_panic() {
    let diagnostics = StandbyDiagnostics::default();
    diagnostics.record_result_offered(StandbySlotId::new(200));
    assert!(diagnostics.snapshot().is_empty());
}
