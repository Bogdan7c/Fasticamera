//! Вердикт `gpu-fence`: проходной результат и каждое правило отказа.

use super::*;

fn summary() -> FenceCheckSummary {
    FenceCheckSummary {
        expected_value: 0xAABB_CCDD,
        work_a_value: 0xAABB_CCDD,
        work_b_value: 0xAABB_CCDD,
        b_started_after_a_finished: true,
        work_a_duration_ns: 14_000_000,
        work_b_duration_ns: 20_000,
        gap_between_a_end_and_b_begin_ns: 35_000,
        release_fence_pending_at_export: Some(true),
        exported_fence_was_already_signaled: false,
        already_signaled_wait_semaphore_created: false,
        already_signaled_work_completed: true,
        already_signaled_result_value: 0xAABB_CCDD,
        validation_errors: 0,
        known_deviations: Default::default(),
        validation: ValidationState::Enabled,
    }
}

fn verdict_with(change: impl FnOnce(&mut FenceCheckSummary)) -> Verdict {
    let mut value = summary();
    change(&mut value);
    evaluate(500_000, &FenceCheckResult::Completed(value))
}

#[test]
fn correct_order_passes_and_report_has_durations_and_gap() {
    let verdict = verdict_with(|_| {});
    assert_eq!(verdict.violations, Vec::<String>::new());
    assert_eq!(verdict.report["work_a_duration_ns"], 14_000_000);
    assert_eq!(verdict.report["gap_between_a_end_and_b_begin_ns"], 35_000);
    assert_eq!(verdict.report["busy_iterations"], 500_000);
}

#[test]
fn b_not_seeing_a_result_fails() {
    let verdict = verdict_with(|s| s.work_b_value = 0xDEAD_BEEF);
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("порядок не соблюдён"))
    );
}

#[test]
fn b_started_before_a_finished_fails() {
    let verdict = verdict_with(|s| {
        s.b_started_after_a_finished = false;
        s.gap_between_a_end_and_b_begin_ns = -1_000;
    });
    assert!(
        verdict
            .violations
            .iter()
            .any(|text| text.contains("до конца A") && text.contains("-1000"))
    );
}

#[test]
fn fence_that_was_not_pending_or_not_checkable_fails() {
    assert!(
        verdict_with(|s| s.release_fence_pending_at_export = Some(false))
            .violations
            .iter()
            .any(|text| text.contains("уже сигнален"))
    );
    assert!(
        verdict_with(|s| s.release_fence_pending_at_export = None)
            .violations
            .iter()
            .any(|text| text.contains("не поддерживается"))
    );
}

#[test]
fn already_signaled_phase_problems_fail() {
    assert_eq!(
        verdict_with(|s| s.already_signaled_wait_semaphore_created = true)
            .violations
            .len(),
        1
    );
    assert_eq!(
        verdict_with(|s| s.already_signaled_work_completed = false)
            .violations
            .len(),
        1
    );
    assert_eq!(
        verdict_with(|s| s.already_signaled_result_value = 0).violations.len(),
        1
    );
}

#[test]
fn validation_errors_fail_only_when_enabled() {
    assert!(
        verdict_with(|s| s.validation_errors = 1)
            .violations
            .iter()
            .any(|text| text.contains("validation"))
    );
    let release = verdict_with(|s| {
        s.validation = ValidationState::Disabled;
        s.validation_errors = 0;
    });
    assert_eq!(release.violations, Vec::<String>::new());
    assert_eq!(release.report["validation"], "disabled");
}

#[test]
fn diagnostic_error_fails_with_reason_and_keeps_report_shape() {
    let verdict = evaluate(
        500_000,
        &FenceCheckResult::Failed {
            reason: "нет VK_KHR_external_fence_fd".to_owned(),
        },
    );
    assert!(verdict.violations.iter().any(|text| text.contains("external_fence_fd")));
    assert_eq!(verdict.report["error"], "нет VK_KHR_external_fence_fd");
}
