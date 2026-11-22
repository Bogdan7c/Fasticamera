//! Тесты журнала выборов кадра: запись/чтение, ограниченность, замена повтора.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_contract::capture_result::SensorTimestamp;
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::{FrameChoice, SubstitutionReason};
use still_contract::plan::StillId;

use crate::still_choice_log::StillChoiceLog;

fn exact(sensor: i64) -> FrameChoice {
    FrameChoice::ShownAtPress {
        sensor_timestamp: SensorTimestamp::from_nanos(sensor),
        shown_at: MonotonicNanos::from_nanos(sensor + 1),
    }
}

#[test]
fn an_unrecorded_still_has_no_choice_and_a_recorded_one_returns_exactly_what_was_written() {
    let log = StillChoiceLog::new(4);
    assert_eq!(log.choice_of(StillId(1)), None);
    let substituted = FrameChoice::Substituted {
        sensor_timestamp: SensorTimestamp::from_nanos(7),
        wanted: None,
        reason: SubstitutionReason::ShownFrameEvicted,
    };
    log.record(StillId(1), exact(5));
    log.record(StillId(2), substituted);
    assert_eq!(log.choice_of(StillId(1)), Some(exact(5)));
    assert_eq!(log.choice_of(StillId(2)), Some(substituted));
    assert_eq!(log.choice_of(StillId(3)), None);
}

#[test]
fn overflow_evicts_the_oldest_and_keeps_the_newest() {
    let log = StillChoiceLog::new(2);
    for id in 1..=3 {
        log.record(StillId(id), exact(i64::try_from(id).unwrap()));
    }
    assert_eq!(log.choice_of(StillId(1)), None);
    let kept: Vec<u64> = log.snapshot().iter().map(|entry| entry.still.0).collect();
    assert_eq!(kept, vec![2, 3]);
}

#[test]
fn recording_the_same_still_twice_keeps_one_entry_with_the_latest_choice() {
    let log = StillChoiceLog::new(4);
    log.record(StillId(1), exact(5));
    log.record(StillId(1), exact(9));
    assert_eq!(log.snapshot().len(), 1);
    assert_eq!(log.choice_of(StillId(1)), Some(exact(9)));
}

#[test]
fn zero_capacity_still_keeps_one_entry() {
    let log = StillChoiceLog::new(0);
    log.record(StillId(1), exact(1));
    assert_eq!(log.choice_of(StillId(1)), Some(exact(1)));
}
