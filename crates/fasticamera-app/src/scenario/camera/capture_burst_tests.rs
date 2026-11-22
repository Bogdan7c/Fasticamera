//! Серия `capture`: привязка запросов к нажатиям по метке, расписание, итог по сроку.

use camera_screen::shutter::ShotRequestId;
use still_contract::plan::StillId;

use super::*;
use crate::scenario::camera::capture_observation::{PhaseBook, SubmitCounts};
use crate::still_phase_log::{PhaseEntry, PhaseKind};
use crate::still_test_support::shot;

fn plan(presses: u32) -> BurstPlan {
    BurstPlan {
        presses,
        interval_ms: 100,
        drain_timeout_ms: 1_000,
        ..BurstPlan::default()
    }
}

fn baselines() -> BurstBaselines {
    BurstBaselines {
        submit: SubmitCounts::default(),
        actions_rejected: 0,
        live: LiveCounters::default(),
        last_request: 5,
    }
}

fn observation<'a>(now: Instant, at_ms: i64, journal: &'a [ShotRequest], accepted: u64) -> CaptureObservation<'a> {
    CaptureObservation {
        now,
        render_now: MonotonicNanos::from_nanos(at_ms * 1_000_000),
        raw_setting: camera_screen::settings::RawCapture::Off,
        actions_rejected: 0,
        submit: SubmitCounts { accepted, rejected: 0 },
        journal,
        live: LiveCounters::default(),
        mode_is_real: true,
    }
}

fn request_at(id: u64, at_ms: i64) -> ShotRequest {
    ShotRequest {
        pressed_at: MonotonicNanos::from_nanos(at_ms * 1_000_000),
        ..shot(id)
    }
}

fn saved_entry(request: u64, at: Instant) -> PhaseEntry {
    PhaseEntry {
        seq: request,
        still: StillId(request),
        request: ShotRequestId(request),
        at,
        kind: PhaseKind::Saved {
            primary: format!("content://{request}"),
            raw: None,
        },
    }
}

#[test]
fn presses_follow_the_interval_and_each_carries_the_render_time_of_its_moment() {
    let base = Instant::now();
    let mut burst = BurstState::new(plan(3), Duration::ZERO, baselines(), base);
    let book = PhaseBook::default();
    let mut presses = Vec::new();

    for step_ms in [0_u64, 50, 100, 150, 200, 250] {
        let obs = observation(base + Duration::from_millis(step_ms), 1_000 + step_ms as i64, &[], 0);
        let (actions, _) = burst.tick(&obs, &book);
        for action in actions {
            if let ScreenAction::ShutterTapped { at } = action {
                presses.push((step_ms, at.as_nanos() / 1_000_000));
            }
        }
    }

    assert_eq!(
        presses,
        vec![(0, 1_000), (100, 1_100), (200, 1_200)],
        "нажатия на 0, 100, 200 мс с метками своего кадра"
    );
}

#[test]
fn a_request_is_bound_to_the_press_with_the_same_time_and_old_journal_entries_are_ignored() {
    let base = Instant::now();
    let mut burst = BurstState::new(plan(1), Duration::ZERO, baselines(), base);
    let mut book = PhaseBook::default();
    // В журнале: чужой запрос из прошлого шага (номер ≤ 5) с той же меткой и наш запрос 6.
    let journal = vec![request_at(5, 1_000), request_at(6, 1_000)];
    book.extend(vec![saved_entry(6, base + Duration::from_millis(200))], 0);

    let (_, progress) = burst.tick(&observation(base, 1_000, &journal, 1), &book);
    assert_eq!(progress, BurstProgress::Settled, "один принятый запрос, итог получен");
    let (record, shots) = burst.finish(&observation(base, 1_000, &journal, 1), &book, "burst", 3);

    assert_eq!(shots.len(), 1);
    assert_eq!(
        (shots[0].number, shots[0].request),
        (3, 6),
        "запрос 5 — чужой, в серию не попал"
    );
    assert_eq!(record.accepted_missing_from_journal, 0);
}

#[test]
fn an_accepted_request_missing_from_the_journal_is_counted_and_blocks_settling() {
    let base = Instant::now();
    let mut burst = BurstState::new(plan(1), Duration::ZERO, baselines(), base);
    let book = PhaseBook::default();

    // Исполнитель экрана принял запрос, а в журнале его нет: ждать нечего, потеря видна по счётчику.
    let (_, progress) = burst.tick(&observation(base, 1_000, &[], 1), &book);
    assert_eq!(progress, BurstProgress::Running);
    let timed_out = burst
        .tick(&observation(base + Duration::from_millis(1_500), 2_500, &[], 1), &book)
        .1;
    assert_eq!(timed_out, BurstProgress::DrainTimedOut);
    let (record, shots) = burst.finish(
        &observation(base + Duration::from_millis(1_500), 2_500, &[], 1),
        &book,
        "burst",
        1,
    );
    assert!(shots.is_empty());
    assert_eq!(record.accepted_missing_from_journal, 1);
}

#[test]
fn settling_waits_at_least_the_minimum_drain_after_the_last_press() {
    let base = Instant::now();
    let mut burst = BurstState::new(plan(1), Duration::from_millis(500), baselines(), base);
    let mut book = PhaseBook::default();
    let journal = vec![request_at(6, 1_100)];
    book.extend(vec![saved_entry(6, base)], 0);

    let early = burst
        .tick(
            &observation(base + Duration::from_millis(100), 1_100, &journal, 1),
            &book,
        )
        .1;
    let late = burst
        .tick(
            &observation(base + Duration::from_millis(600), 1_600, &journal, 1),
            &book,
        )
        .1;

    assert_eq!((early, late), (BurstProgress::Running, BurstProgress::Settled));
}
