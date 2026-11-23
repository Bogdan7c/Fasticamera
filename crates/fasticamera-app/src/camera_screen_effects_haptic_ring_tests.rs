//! Тесты кольца «недавних вибраций» исполнителя эффектов: принятые вибрации запоминаются со временем кадра, кольцо
//! вытесняет старые, но счёт не теряет, отвергнутые в кольцо не попадают. Стенд (`build_executor`, `Behavior`) — из
//! `camera_screen_effects_tests`.

use super::*;

#[test]
fn accepted_haptics_are_remembered_with_their_frame_times_oldest_first() {
    let (mut executor, _rig) = build_executor(Behavior::default());
    let play = |executor: &mut EffectExecutor, at_ms: i64| {
        executor.begin_frame(MonotonicNanos::from_nanos(at_ms * 1_000_000));
        executor.execute(vec![Effect::Haptic(HapticPattern::SelectionTick)]);
    };

    for at_ms in [100, 160, 190, 400] {
        play(&mut executor, at_ms);
    }

    let recent = executor.counters().haptics.recent_queued;
    assert_eq!(executor.counters().haptics.queued, 4);
    assert_eq!(recent.total(), 4);
    let times: Vec<i64> = recent
        .retained()
        .iter()
        .map(|time| time.as_nanos() / 1_000_000)
        .collect();
    assert_eq!(times, vec![100, 160, 190, 400]);
}

#[test]
fn the_haptic_ring_keeps_only_the_newest_times_but_still_counts_all() {
    let (mut executor, _rig) = build_executor(Behavior::default());
    let total = RECENT_HAPTICS_CAPACITY as i64 + 5;
    for at_ms in 0..total {
        executor.begin_frame(MonotonicNanos::from_nanos(at_ms * 1_000_000));
        executor.execute(vec![Effect::Haptic(HapticPattern::SelectionTick)]);
    }

    let recent = executor.counters().haptics.recent_queued;
    assert_eq!(recent.total(), total as u64, "счёт не теряется при вытеснении");
    let times: Vec<i64> = recent
        .retained()
        .iter()
        .map(|time| time.as_nanos() / 1_000_000)
        .collect();
    assert_eq!(times.len(), RECENT_HAPTICS_CAPACITY);
    assert_eq!(times.first().copied(), Some(5), "самые старые вытеснены");
    assert_eq!(times.last().copied(), Some(total - 1));
}

#[test]
fn rejected_haptics_are_not_remembered_as_played() {
    let (mut executor, _rig) = build_executor(Behavior {
        haptics: HapticOutcome::Backpressure,
        ..Behavior::default()
    });
    for at_ms in [10, 11, 12] {
        executor.begin_frame(MonotonicNanos::from_nanos(at_ms * 1_000_000));
        executor.execute(vec![Effect::Haptic(HapticPattern::SelectionTick)]);
    }
    let haptics = executor.counters().haptics;
    assert_eq!((haptics.queued, haptics.backpressure), (0, 3));
    assert_eq!(
        haptics.recent_queued.total(),
        0,
        "отвергнутые вибрации в кольцо не попадают"
    );
}
