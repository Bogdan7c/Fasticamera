//! Тесты «снимка без задержки»: перевод часов, точный и заменённый выбор, неизвестный выбор, сводка.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_contract::capture_result::SensorTimestamp;
use shell_contract::clock::{BoottimeNanos, MonotonicNanos};
use still_contract::frame_choice::{FrameChoice, SubstitutionReason};

use super::{ClockBridge, FrameChoiceView, FrameSummary, FrameTiming};
use crate::clock_offset::ClockOffset;

const MS: i64 = 1_000_000;
/// Телефон проспал: BOOTTIME впереди MONOTONIC на 1000 с — смешать шкалы без моста значило бы получить мусор.
const BOOT_AHEAD_NANOS: i64 = 1_000_000_000_000;
const PRESS_NANOS: i64 = 5_000_000_000;

fn bridge() -> ClockBridge {
    ClockBridge::Available(ClockOffset::between(
        BoottimeNanos::from_nanos(PRESS_NANOS + BOOT_AHEAD_NANOS),
        MonotonicNanos::from_nanos(PRESS_NANOS),
    ))
}

fn press() -> MonotonicNanos {
    MonotonicNanos::from_nanos(PRESS_NANOS)
}

/// Кадр, чья экспозиция началась за `before_ms` до нажатия (метка в BOOTTIME).
fn sensor_before_press(before_ms: i64) -> SensorTimestamp {
    SensorTimestamp::from_nanos(PRESS_NANOS + BOOT_AHEAD_NANOS - before_ms * MS)
}

fn exact(before_ms: i64, shown_before_ms: i64) -> FrameChoice {
    FrameChoice::ShownAtPress {
        sensor_timestamp: sensor_before_press(before_ms),
        shown_at: MonotonicNanos::from_nanos(PRESS_NANOS - shown_before_ms * MS),
    }
}

fn substituted(before_ms: i64, reason: SubstitutionReason) -> FrameChoice {
    FrameChoice::Substituted {
        sensor_timestamp: sensor_before_press(before_ms),
        wanted: None,
        reason,
    }
}

#[test]
fn exact_choice_gets_delay_through_the_clock_bridge_and_time_on_screen_before_the_press() {
    let timing = FrameTiming::of(Some(exact(80, 12)), "n/a", press(), &bridge());
    assert_eq!(timing.frame_choice, FrameChoiceView::Exact);
    assert_eq!(timing.press_to_frame_ms, Some(80.0));
    assert_eq!(timing.shown_before_press_ms, Some(12.0));
    assert_eq!(timing.press_to_frame_unavailable, None);
}

#[test]
fn without_the_bridge_the_delay_is_null_with_a_reason_but_the_choice_is_still_reported() {
    let clock = ClockBridge::Unavailable {
        reason: "errno 1".to_owned(),
    };
    let timing = FrameTiming::of(Some(exact(80, 12)), "n/a", press(), &clock);
    assert_eq!(timing.frame_choice, FrameChoiceView::Exact);
    assert_eq!(timing.press_to_frame_ms, None);
    assert!(
        timing
            .press_to_frame_unavailable
            .as_deref()
            .unwrap()
            .contains("errno 1")
    );
    assert_eq!(
        timing.shown_before_press_ms,
        Some(12.0),
        "показ — в одной шкале, мост не нужен"
    );
}

#[test]
fn substituted_choice_keeps_its_reason_has_no_shown_time_and_a_later_frame_gives_a_negative_delay() {
    let timing = FrameTiming::of(
        Some(substituted(-5, SubstitutionReason::ShownFrameEvicted)),
        "n/a",
        press(),
        &bridge(),
    );
    assert_eq!(
        timing.frame_choice,
        FrameChoiceView::Substituted {
            reason: SubstitutionReason::ShownFrameEvicted
        }
    );
    assert_eq!(timing.shown_before_press_ms, None);
    assert_eq!(timing.press_to_frame_ms, Some(-5.0));
    assert!(timing.is_evicted_substitution() && !timing.is_exact());
}

#[test]
fn a_missing_choice_is_unavailable_and_never_invents_a_delay() {
    let timing = FrameTiming::of(None, "журнал вытеснил запись", press(), &bridge());
    assert_eq!(
        timing.frame_choice,
        FrameChoiceView::Unavailable {
            why: "журнал вытеснил запись"
        }
    );
    assert!(!timing.choice_known());
    assert_eq!(timing.press_to_frame_ms, None);
}

#[test]
fn summary_counts_shares_and_percentiles_over_known_choices_only() {
    let timings = [
        FrameTiming::of(Some(exact(70, 0)), "", press(), &bridge()),
        FrameTiming::of(Some(exact(90, 0)), "", press(), &bridge()),
        FrameTiming::of(Some(exact(80, 0)), "", press(), &bridge()),
        FrameTiming::of(
            Some(substituted(100, SubstitutionReason::PresentationTimingUnknown)),
            "",
            press(),
            &bridge(),
        ),
        FrameTiming::of(None, "нет записи", press(), &bridge()),
    ];
    let summary = FrameSummary::of(&timings);
    assert_eq!((summary.choices_known, summary.choices_unknown), (4, 1));
    assert_eq!(
        (summary.exact, summary.substituted, summary.substituted_evicted),
        (3, 1, 0)
    );
    assert_eq!(summary.exact_share, Some(0.75));
    assert_eq!(summary.delays_measured, 4);
    assert_eq!(summary.max_press_to_frame_ms, Some(100.0));
    assert_eq!(
        summary.p50_press_to_frame_ms,
        Some(80.0),
        "70, 80, 90, 100 → нижняя медиана 80"
    );
    assert!(
        summary.delay_unavailable_reason.is_some(),
        "у снимка без выбора причина названа"
    );
}

#[test]
fn summary_of_nothing_has_no_share_and_no_percentiles() {
    let summary = FrameSummary::of(std::iter::empty());
    assert_eq!(summary.exact_share, None);
    assert_eq!(
        (summary.max_press_to_frame_ms, summary.p50_press_to_frame_ms),
        (None, None)
    );
}
