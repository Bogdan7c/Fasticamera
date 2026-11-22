//! Приращение диагностики снимков рендера: берётся разность, а не накопленное; перезапуск бэкенда не даёт минусов.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use render_loop::still_diagnostics::StillDiagnostics;

use super::StillDiagnosticsDelta;

#[test]
fn delta_is_the_difference_per_kind_and_ignores_what_was_there_before_the_run() {
    let mut before = StillDiagnostics::default();
    before.picks.exact = 5;
    before.picks.substituted_evicted = 2;
    before.gpu_lost = 1;
    before.exact_press_after_shown.observe(10.0);

    let mut after = before.clone();
    after.begins = 4;
    after.picks.exact += 3;
    after.picks.substituted_timing_unknown += 1;
    after.picks.no_frame_ring_empty += 2;
    after.refusals.queue_full += 1;
    after.refusals.capture_failed += 2;
    after.outcomes.rendered += 3;
    after.outcomes.failed_gpu += 1;
    after.exact_press_after_shown.observe(20.0);
    after.exact_press_after_shown.observe(30.0);

    let delta = StillDiagnosticsDelta::between(&before, &after);
    assert_eq!(delta.begins, 4);
    assert_eq!(delta.picks.exact, 3);
    assert_eq!(delta.picks.substituted_timing_unknown, 1);
    assert_eq!(
        delta.picks.substituted_evicted, 0,
        "прежние 2 вытеснения к прогону не относятся"
    );
    assert_eq!(delta.picks.no_frame, 2);
    assert_eq!(delta.refusals, 3);
    assert_eq!(delta.gpu_lost, 0, "прежняя потеря GPU к прогону не относится");
    assert_eq!((delta.completed, delta.failed), (3, 1));
    assert_eq!(delta.exact_press_after_shown_count, 2);
    assert_eq!(delta.exact_press_after_shown_mean_ms, Some(25.0));
}

#[test]
fn a_restarted_backend_never_produces_negative_or_wrapped_numbers() {
    let mut before = StillDiagnostics::default();
    before.picks.exact = 9;
    before.gpu_lost = 4;
    let after = StillDiagnostics::default();
    let delta = StillDiagnosticsDelta::between(&before, &after);
    assert_eq!((delta.picks.exact, delta.gpu_lost), (0, 0));
    assert_eq!(delta.exact_press_after_shown_mean_ms, None);
}

#[test]
fn flash_delta_counts_only_this_run_and_sums_every_undecidable_reason() {
    let mut before = StillDiagnostics::default();
    before.flash.lit_check_passed = 4;
    before.flash.not_lit = 1;
    before.flash.lit_check_skipped_low_predicted_share = 2;

    let mut after = before.clone();
    after.flash.begins = 2;
    after.flash.frames_found = 2;
    after.flash.lit_check_passed += 1;
    after.flash.lit_check_skipped_low_predicted_share += 1;
    after.flash.lit_check_skipped_no_ambient_level += 1;
    after.flash.white_from_torch_frame = 2;

    let flash = StillDiagnosticsDelta::between(&before, &after).flash;
    assert_eq!(flash.begins, 2);
    assert_eq!(flash.frames_found, 2);
    assert_eq!(flash.lit_check_passed, 1, "прежние 4 проверки к прогону не относятся");
    assert_eq!(flash.not_lit, 0, "прежний неосвещённый кадр к прогону не относится");
    assert_eq!(flash.lit_check_skipped, 2, "обе причины «не решаемо» сложены");
    assert_eq!(flash.white_from_torch_frame, 2);
}
