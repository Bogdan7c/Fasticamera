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

/// Все события, наступившие к `elapsed_ms`, как простой список (для коротких проверок).
fn events_until(timeline: &Timeline, elapsed_ms: u64) -> Vec<ScheduledEvent> {
    timeline.events_due(0, elapsed_ms).to_vec()
}

#[test]
fn events_are_handed_out_once_in_time_order() {
    let timeline = default_timeline();
    let mut fired = 0;

    let first = timeline.events_due(fired, 0).to_vec();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].event, PlanEvent::PhaseStart(TracePhase::Photo));
    fired += first.len();

    // Первый шаг отсчёта «3» наступает за три секунды до подсказки, не раньше.
    assert!(timeline.events_due(fired, 2_999).is_empty());
    let countdown_start = timeline.events_due(fired, 3_000).to_vec();
    assert_eq!(
        countdown_start[0].event,
        PlanEvent::Countdown {
            phase: TracePhase::Photo,
            index: 0,
            seconds_left: 3
        }
    );
    fired += countdown_start.len();

    // Тик, пропустивший сразу несколько моментов, выдаёт их все и по порядку.
    let burst = timeline.events_due(fired, 6_000).to_vec();
    assert_eq!(
        burst.iter().map(|scheduled| scheduled.event).collect::<Vec<_>>(),
        vec![
            PlanEvent::Countdown {
                phase: TracePhase::Photo,
                index: 0,
                seconds_left: 2
            },
            PlanEvent::Countdown {
                phase: TracePhase::Photo,
                index: 0,
                seconds_left: 1
            },
            PlanEvent::Cue {
                phase: TracePhase::Photo,
                index: 0
            },
        ]
    );
    fired += burst.len();
    assert!(timeline.events_due(fired, 6_000).is_empty(), "повторно не выдаются");
}

#[test]
fn cue_moments_do_not_depend_on_the_countdown() {
    let with_countdown = default_timeline();
    let without_countdown = Timeline::from_config(&AeTraceScenarioConfig {
        cue_countdown_steps: 0,
        ..AeTraceScenarioConfig::default()
    });
    let cue_moments = |timeline: &Timeline| -> Vec<(u64, PlanEvent)> {
        events_until(timeline, timeline.total_ms())
            .into_iter()
            .filter(|scheduled| matches!(scheduled.event, PlanEvent::Cue { .. }))
            .map(|scheduled| (scheduled.at_ms, scheduled.event))
            .collect()
    };
    assert_eq!(cue_moments(&with_countdown), cue_moments(&without_countdown));
    assert_eq!(cue_moments(&with_countdown).len(), 10);
    assert_eq!(
        with_countdown.phases(),
        without_countdown.phases(),
        "окна разбора те же"
    );
}

#[test]
fn countdown_ticks_run_before_each_cue_and_the_act_label_is_released_afterwards() {
    let timeline = default_timeline();
    let around_second_cue: Vec<(u64, PlanEvent)> = events_until(&timeline, 14_000)
        .into_iter()
        .filter(|scheduled| scheduled.at_ms > 8_000)
        .map(|scheduled| (scheduled.at_ms, scheduled.event))
        .collect();
    let photo = TracePhase::Photo;
    assert_eq!(
        around_second_cue,
        vec![
            (
                9_000,
                PlanEvent::Countdown {
                    phase: photo,
                    index: 1,
                    seconds_left: 3
                }
            ),
            (
                10_000,
                PlanEvent::Countdown {
                    phase: photo,
                    index: 1,
                    seconds_left: 2
                }
            ),
            (
                11_000,
                PlanEvent::Countdown {
                    phase: photo,
                    index: 1,
                    seconds_left: 1
                }
            ),
            (12_000, PlanEvent::Cue { phase: photo, index: 1 }),
            (14_000, PlanEvent::CueShown { phase: photo, index: 1 }),
        ]
    );
}

#[test]
fn cue_label_release_is_dropped_when_it_would_land_in_the_next_phase() {
    // Фаза 7 с, подсказка в 6 с: надпись держалась бы до 8 с, за границей фазы — её заменит старт следующей.
    let config = AeTraceScenarioConfig {
        photo_phase_ms: 7_000,
        ..AeTraceScenarioConfig::default()
    };
    let timeline = Timeline::from_config(&config);
    let shown: Vec<_> = events_until(&timeline, timeline.total_ms())
        .into_iter()
        .filter(|scheduled| {
            matches!(
                scheduled.event,
                PlanEvent::CueShown {
                    phase: TracePhase::Photo,
                    ..
                }
            )
        })
        .collect();
    assert!(shown.is_empty(), "{shown:?}");
}

#[test]
fn cue_counts_per_phase_feed_the_step_x_of_n_labels() {
    let timeline = default_timeline();
    assert_eq!(timeline.cue_count(TracePhase::Photo), 5);
    assert_eq!(timeline.cue_count(TracePhase::Video), 5);
    assert_eq!(timeline.cue_count(TracePhase::Static), 0);
}

#[test]
fn all_events_are_eventually_delivered_exactly_once() {
    let timeline = default_timeline();
    let all = timeline.events_due(0, timeline.total_ms());
    // 3 начала фаз + по 5 событий (3 отсчёта, подсказка, возврат «жди») на каждую из 10 подсказок.
    assert_eq!(all.len(), 3 + 10 * 5);
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
