#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::time::Duration;

use super::*;

#[test]
fn offset_is_converted_to_whole_minutes() {
    assert_eq!(moment_from(1_000, Some(3 * 3600 + 30 * 60)).utc_offset_minutes, 210);
    assert_eq!(moment_from(1_000, Some(-5 * 3600)).utc_offset_minutes, -300);
    assert_eq!(moment_from(1_000, Some(0)).utc_offset_minutes, 0);
}

#[test]
fn unknown_zone_is_utc_and_keeps_the_instant() {
    let moment = moment_from(1_700_000_123_456, None);
    assert_eq!((moment.utc_millis, moment.utc_offset_minutes), (1_700_000_123_456, 0));
}

#[test]
fn unix_millis_handles_both_sides_of_the_epoch() {
    assert_eq!(unix_millis(UNIX_EPOCH + Duration::from_millis(1_500)), 1_500);
    assert_eq!(unix_millis(UNIX_EPOCH - Duration::from_millis(2_000)), -2_000);
}

#[test]
fn system_clock_tracks_real_time_and_reports_a_sane_zone() {
    let before = unix_millis(SystemTime::now());
    let moment = SystemWallClock::default().now();
    let after = unix_millis(SystemTime::now());
    assert!(
        (before..=after).contains(&moment.utc_millis),
        "UTC взят из системных часов"
    );
    // Пояса Земли лежат в пределах от −12 до +14 часов.
    assert!((-12 * 60..=14 * 60).contains(&moment.utc_offset_minutes), "{moment:?}");
}
