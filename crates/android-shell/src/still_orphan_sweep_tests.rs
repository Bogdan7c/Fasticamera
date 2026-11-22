#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use super::*;

#[test]
fn every_platform_state_is_decoded() {
    assert_eq!(
        OrphanSweepReport::from_platform_text("not_started"),
        OrphanSweepReport::NotStarted
    );
    assert_eq!(
        OrphanSweepReport::from_platform_text("running"),
        OrphanSweepReport::Running
    );
    assert_eq!(
        OrphanSweepReport::from_platform_text("removed:3"),
        OrphanSweepReport::Removed { count: 3 }
    );
    assert_eq!(
        OrphanSweepReport::from_platform_text("removed:0"),
        OrphanSweepReport::Removed { count: 0 },
        "ноль сирот — нормальный итог, не отсутствие уборки"
    );
    assert_eq!(
        OrphanSweepReport::from_platform_text("failed: SecurityException: нет доступа"),
        OrphanSweepReport::Failed {
            reason: "SecurityException: нет доступа".to_owned()
        }
    );
}

#[test]
fn garbage_is_a_failure_with_the_text_not_a_silent_state() {
    for garbage in ["", "removed:-1", "removed:abc", "done"] {
        match OrphanSweepReport::from_platform_text(garbage) {
            OrphanSweepReport::Failed { reason } => assert!(reason.contains(garbage), "{reason}"),
            other => panic!("`{garbage}` разобран как {other:?}"),
        }
    }
}

#[test]
fn report_serializes_with_a_state_tag() {
    let json = serde_json::to_string(&OrphanSweepReport::Removed { count: 2 }).unwrap();
    assert_eq!(json, r#"{"state":"removed","count":2}"#);
}
