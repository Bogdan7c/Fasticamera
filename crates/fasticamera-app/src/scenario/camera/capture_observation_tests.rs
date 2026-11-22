//! Срезы наблюдения `capture`: приращения счётчиков и сборка записи снимка из журналов запроса и службы.

use std::time::Duration;

use camera_screen::settings::FlashMode;
use camera_screen::shutter::{PrimaryFormat, ShotRequestId};
use still_contract::plan::StillId;

use super::*;
use crate::still_phase_log::PhaseKind;
use crate::still_test_support::shot;

fn entry(seq: u64, request: u64, still: u64, at: Instant, kind: PhaseKind) -> PhaseEntry {
    PhaseEntry {
        seq,
        still: StillId(still),
        request: ShotRequestId(request),
        at,
        kind,
    }
}

fn no_clock() -> ClockBridge {
    ClockBridge::Unavailable {
        reason: "в этом тесте часы не нужны".to_owned(),
    }
}

fn saved(primary: &str, raw: Option<&str>) -> PhaseKind {
    PhaseKind::Saved {
        primary: primary.to_owned(),
        raw: raw.map(str::to_owned),
    }
}

#[test]
fn live_counters_give_deltas_and_never_go_negative_after_a_renderer_restart() {
    let before = LiveCounters {
        jitter_overflow: 5,
        late_frames: 10,
        holds_started: 2,
        ..LiveCounters::default()
    };
    let after = LiveCounters {
        jitter_overflow: 7,
        late_frames: 4, // счётчик начался заново
        holds_started: 2,
        live_frames_presented: 90,
        ..LiveCounters::default()
    };

    let delta = after.delta_since(before);

    assert_eq!(delta.jitter_overflow, 2);
    assert_eq!(delta.late_frames, 0, "насыщение, а не переполнение");
    assert_eq!(delta.holds_started, 0);
    assert_eq!(delta.live_frames_presented, 90);
}

#[test]
fn submit_counts_split_attempts_into_accepted_and_rejected() {
    let counts = SubmitCounts {
        accepted: 7,
        rejected: 3,
    };
    assert_eq!(counts.submitted(), 10);
    assert_eq!(
        counts.delta_since(SubmitCounts {
            accepted: 2,
            rejected: 3
        }),
        SubmitCounts {
            accepted: 5,
            rejected: 0
        }
    );
}

#[test]
fn a_saved_shot_gets_both_addresses_both_latencies_and_the_request_format() {
    let pressed = Instant::now();
    let mut book = PhaseBook::default();
    book.extend(
        vec![
            entry(1, 4, 40, pressed + Duration::from_millis(35), PhaseKind::Secured),
            entry(
                2,
                4,
                40,
                pressed + Duration::from_millis(900),
                saved("content://a", Some("content://a-raw")),
            ),
        ],
        0,
    );
    let request = ShotRequest {
        raw: camera_screen::settings::RawCapture::On,
        primary_format: PrimaryFormat::Heif,
        ..shot(4)
    };

    let record = shot_record(
        3,
        "single_heif_dng",
        ShotExpectation {
            raw: true,
            flash: FlashMode::Off,
        },
        &request,
        pressed,
        &book,
        &no_clock(),
    );

    assert_eq!(record.number, 3);
    assert_eq!(record.request, 4);
    assert_eq!(record.still, Some(40));
    assert_eq!((record.secured_after_ms, record.saved_after_ms), (Some(35), Some(900)));
    assert_eq!(
        record.outcome,
        ShotOutcome::Saved {
            primary: "content://a".to_owned(),
            raw: Some("content://a-raw".to_owned())
        }
    );
    assert!(record.raw_requested && record.raw_expected);
    assert_eq!(
        (record.format, record.aspect, record.zoom_milli),
        ("heif", "4:3", 1_000)
    );
}

#[test]
fn a_failed_shot_keeps_the_service_text_and_has_no_save_time() {
    let pressed = Instant::now();
    let mut book = PhaseBook::default();
    book.extend(
        vec![entry(
            1,
            9,
            90,
            pressed + Duration::from_millis(50),
            PhaseKind::Failed {
                failure: "кодирование: HEIC".to_owned(),
            },
        )],
        0,
    );

    let record = shot_record(
        1,
        "single_heif",
        ShotExpectation::PLAIN,
        &shot(9),
        pressed,
        &book,
        &no_clock(),
    );

    assert_eq!(
        record.outcome,
        ShotOutcome::Failed {
            failure: "кодирование: HEIC".to_owned()
        }
    );
    assert_eq!((record.secured_after_ms, record.saved_after_ms), (None, None));
    assert_eq!(record.still, Some(90), "номер снимка службы известен и по срыву");
}

#[test]
fn an_unfinished_shot_says_whether_the_frame_was_at_least_captured() {
    let pressed = Instant::now();
    let mut book = PhaseBook::default();
    book.extend(vec![entry(1, 2, 20, pressed, PhaseKind::Secured)], 0);

    let captured = shot_record(
        1,
        "burst",
        ShotExpectation::PLAIN,
        &shot(2),
        pressed,
        &book,
        &no_clock(),
    );
    let silent = shot_record(
        2,
        "burst",
        ShotExpectation::PLAIN,
        &shot(3),
        pressed,
        &book,
        &no_clock(),
    );

    assert_eq!(captured.outcome, ShotOutcome::Unfinished { secured: true });
    assert_eq!(silent.outcome, ShotOutcome::Unfinished { secured: false });
    assert_eq!(silent.still, None);
}

#[test]
fn phases_of_other_requests_do_not_leak_into_a_shot_and_the_book_counts_lost_entries() {
    let pressed = Instant::now();
    let mut book = PhaseBook::default();
    book.extend(vec![entry(1, 1, 10, pressed, saved("content://one", None))], 2);
    book.extend(Vec::new(), 3);

    assert_eq!(book.lost(), 5);
    assert!(book.terminal_of(2).is_none());
    assert!(book.terminal_of(1).is_some());
    assert!(book.secured_of(1).is_none());
}

#[test]
fn aspect_names_cover_every_ratio() {
    assert_eq!(aspect_name(AspectRatio::FourByThree), "4:3");
    assert_eq!(aspect_name(AspectRatio::SixteenByNine), "16:9");
    assert_eq!(aspect_name(AspectRatio::Square), "1:1");
}

#[test]
fn a_shot_takes_the_frame_choice_of_its_own_still_and_never_of_a_neighbour() {
    use camera_contract::capture_result::SensorTimestamp;
    use shell_contract::clock::{BoottimeNanos, MonotonicNanos};
    use still_contract::frame_choice::{FrameChoice, SubstitutionReason};

    use super::super::capture_frame_timing::FrameChoiceView;
    use crate::clock_offset::ClockOffset;
    use crate::still_choice_log::ChoiceEntry;

    let pressed = Instant::now();
    let mut book = PhaseBook::default();
    book.extend(
        vec![
            entry(1, 1, 10, pressed, PhaseKind::Secured),
            entry(2, 2, 20, pressed, PhaseKind::Secured),
        ],
        0,
    );
    // Снимок 10 — ровно показанный кадр, снимок 20 — замена из-за вытеснения; у запроса 3 снимка службы ещё нет.
    let press_nanos = 9_000_000_000_i64;
    let sensor = |before_ms: i64| SensorTimestamp::from_nanos(press_nanos + 500 - before_ms * 1_000_000);
    book.absorb_choices(vec![
        ChoiceEntry {
            still: StillId(10),
            choice: FrameChoice::ShownAtPress {
                sensor_timestamp: sensor(80),
                shown_at: MonotonicNanos::from_nanos(press_nanos - 5_000_000),
            },
        },
        ChoiceEntry {
            still: StillId(20),
            choice: FrameChoice::Substituted {
                sensor_timestamp: sensor(120),
                wanted: None,
                reason: SubstitutionReason::ShownFrameEvicted,
            },
        },
    ]);
    let clock = ClockBridge::Available(ClockOffset::between(
        BoottimeNanos::from_nanos(press_nanos + 500),
        MonotonicNanos::from_nanos(press_nanos),
    ));
    let request = |id: u64| ShotRequest {
        pressed_at: MonotonicNanos::from_nanos(press_nanos),
        ..shot(id)
    };

    let first = shot_record(1, "burst", ShotExpectation::PLAIN, &request(1), pressed, &book, &clock);
    let second = shot_record(2, "burst", ShotExpectation::PLAIN, &request(2), pressed, &book, &clock);
    let third = shot_record(3, "burst", ShotExpectation::PLAIN, &request(3), pressed, &book, &clock);

    assert_eq!(first.frame.frame_choice, FrameChoiceView::Exact);
    assert_eq!(first.frame.press_to_frame_ms, Some(80.0));
    assert_eq!(first.frame.shown_before_press_ms, Some(5.0));
    assert!(second.frame.is_evicted_substitution());
    assert_eq!(second.frame.press_to_frame_ms, Some(120.0));
    assert!(
        !third.frame.choice_known(),
        "у запроса без снимка службы чужой выбор не подставляется"
    );
}
