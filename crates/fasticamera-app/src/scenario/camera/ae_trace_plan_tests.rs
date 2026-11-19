//! Расписание `ae-trace`: порядок фаз, места подсказок, выдача событий по времени.

use super::*;

fn default_timeline() -> Timeline {
    Timeline::from_config(&AeTraceScenarioConfig::default())
}

#[test]
fn default_schedule_has_three_phases_in_order_with_five_cues_in_each_cued_phase() {
    let timeline = default_timeline();
    let windows = timeline.phases();
    assert_eq!(
        windows.iter().map(|window| window.phase).collect::<Vec<_>>(),
        TracePhase::ALL
    );
    assert_eq!((windows[0].start_ms, windows[0].end_ms), (0, 36_000));
    assert_eq!((windows[1].start_ms, windows[1].end_ms), (36_000, 72_000));
    assert_eq!((windows[2].start_ms, windows[2].end_ms), (72_000, 92_000));
    assert_eq!(windows[0].cues_ms, vec![6_000, 12_000, 18_000, 24_000, 30_000]);
    assert_eq!(
        windows[1].cues_ms,
        vec![42_000, 48_000, 54_000, 60_000, 66_000],
        "подсказки отсчитываются от начала своей фазы"
    );
    assert!(windows[2].cues_ms.is_empty(), "в static владелец ничего не трогает");
    assert_eq!(timeline.total_ms(), 92_000);
}

#[test]
fn cue_never_lands_on_the_phase_boundary() {
    // Подсказка ровно на границе совпала бы с сигналом начала следующей фазы.
    let timeline = default_timeline();
    for window in timeline.phases() {
        assert!(window.cues_ms.iter().all(|at_ms| *at_ms < window.end_ms), "{window:?}");
        assert!(
            window.cues_ms.iter().all(|at_ms| *at_ms > window.start_ms),
            "{window:?}"
        );
    }
}

#[test]
fn segment_ends_at_the_next_cue_or_at_the_phase_end() {
    let timeline = default_timeline();
    let photo = &timeline.phases()[0];
    assert_eq!(photo.segment_end_ms(0), 12_000);
    assert_eq!(photo.segment_end_ms(4), 36_000, "последняя подсказка — до конца фазы");
}

#[test]
fn events_are_handed_out_once_in_time_order() {
    let timeline = default_timeline();
    let mut fired = 0;

    let first = timeline.events_due(fired, 0).to_vec();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].event, PlanEvent::PhaseStart(TracePhase::Photo));
    fired += first.len();

    assert!(timeline.events_due(fired, 5_999).is_empty());
    let at_cue = timeline.events_due(fired, 6_000).to_vec();
    assert_eq!(at_cue.len(), 1);
    assert_eq!(
        at_cue[0].event,
        PlanEvent::Cue {
            phase: TracePhase::Photo,
            index: 0
        }
    );
    fired += at_cue.len();

    // Тик, пропустивший сразу несколько моментов, выдаёт их все и по порядку.
    let burst = timeline.events_due(fired, 36_000).to_vec();
    assert_eq!(
        burst.last().map(|scheduled| scheduled.event),
        Some(PlanEvent::PhaseStart(TracePhase::Video))
    );
    assert_eq!(burst.len(), 5, "подсказки 2-5 фото и начало видео");
    fired += burst.len();
    assert!(timeline.events_due(fired, 36_000).is_empty(), "повторно не выдаются");
}

#[test]
fn all_events_are_eventually_delivered_exactly_once() {
    let timeline = default_timeline();
    let all = timeline.events_due(0, timeline.total_ms());
    // 3 начала фаз + 5 + 5 подсказок.
    assert_eq!(all.len(), 13);
    assert!(all.windows(2).all(|pair| pair[0].at_ms <= pair[1].at_ms));
    assert!(timeline.events_due(all.len(), u64::MAX).is_empty());
    assert!(!timeline.is_finished(91_999));
    assert!(timeline.is_finished(92_000));
}

#[test]
fn phases_map_to_their_engine_profiles() {
    assert_eq!(TracePhase::Photo.profile(), ThreeAProfile::Photo);
    assert_eq!(TracePhase::Video.profile(), ThreeAProfile::Video);
    assert_eq!(TracePhase::Static.profile(), ThreeAProfile::Photo);
}
