//! Детектор мерцания: когда защита снимается и когда возвращается.

use std::time::Duration;

use camera_contract::three_a::{MainsFrequency, SceneFlicker};

use super::config::FlickerPolicy;
use super::flicker::{FlickerDetector, Protection, half_mains_period};

fn follow(frames: u32) -> FlickerDetector {
    FlickerDetector::new(
        FlickerPolicy::FollowDetector {
            clear_after_frames: frames,
        },
        MainsFrequency::Hz50,
    )
}

#[test]
fn protection_is_cleared_only_after_the_configured_run_of_clear_frames() {
    let mut detector = follow(3);
    assert_eq!(detector.snapshot().period(), Some(Duration::from_millis(10)));
    assert!(!detector.observe(Some(SceneFlicker::None)));
    assert!(!detector.observe(Some(SceneFlicker::None)));
    assert!(
        detector.observe(Some(SceneFlicker::None)),
        "третий чистый кадр снимает защиту"
    );
    assert_eq!(detector.snapshot().protection, Protection::Cleared);
    assert_eq!(detector.snapshot().period(), None);
}

#[test]
fn frames_without_the_key_neither_count_nor_break_the_run() {
    let mut detector = follow(2);
    detector.observe(Some(SceneFlicker::None));
    assert!(!detector.observe(None));
    assert!(
        detector.observe(Some(SceneFlicker::None)),
        "пропуск ключа серию не сбросил"
    );
}

#[test]
fn a_flicker_report_breaks_the_run_and_restores_protection_immediately() {
    let mut detector = follow(2);
    detector.observe(Some(SceneFlicker::None));
    assert!(
        !detector.observe(Some(SceneFlicker::Mains50Hz)),
        "защита и так была, снимок тот же"
    );
    assert!(!detector.observe(Some(SceneFlicker::None)), "серия начата заново");
    assert!(detector.observe(Some(SceneFlicker::None)));
    assert!(
        detector.observe(Some(SceneFlicker::Mains60Hz)),
        "отчёт возвращает защиту и меняет частоту"
    );
    let snapshot = detector.snapshot();
    assert_eq!(snapshot.protection, Protection::Active);
    assert_eq!(snapshot.period(), Some(half_mains_period(MainsFrequency::Hz60)));
    assert_eq!(half_mains_period(MainsFrequency::Hz60), Duration::from_nanos(8_333_333));
}

#[test]
fn new_stream_restores_default_protection_but_keeps_the_learned_frequency() {
    let mut detector = follow(1);
    detector.observe(Some(SceneFlicker::Mains60Hz));
    detector.observe(Some(SceneFlicker::None));
    assert_eq!(detector.snapshot().protection, Protection::Cleared);
    detector.reset_for_new_stream();
    let snapshot = detector.snapshot();
    assert_eq!(snapshot.protection, Protection::Active);
    assert_eq!(snapshot.mains, MainsFrequency::Hz60);
}

#[test]
fn fixed_policies_ignore_the_clear_reports() {
    let mut always = FlickerDetector::new(FlickerPolicy::AlwaysProtect, MainsFrequency::Hz50);
    for _ in 0..100 {
        always.observe(Some(SceneFlicker::None));
    }
    assert_eq!(always.snapshot().protection, Protection::Active);
    assert!(always.observe(Some(SceneFlicker::Mains60Hz)), "частота уточняется");

    let mut never = FlickerDetector::new(FlickerPolicy::NeverProtect, MainsFrequency::Hz50);
    assert_eq!(never.snapshot().period(), None);
    never.observe(Some(SceneFlicker::Mains50Hz));
    assert_eq!(
        never.snapshot().protection,
        Protection::Cleared,
        "осознанное «нет защиты» отчёт не включает"
    );
}
