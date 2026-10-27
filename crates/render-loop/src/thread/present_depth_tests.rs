//! Тесты схода на ближнюю линию (решение 54) через настоящий планировщик: тики идут подряд,
//! «сейчас» каждого тика — момент его vsync, линии — как у Choreographer.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::vsync::{FrameTimelines, VsyncTimeline};

use super::super::present_target::{PresentTargetPlanner, TickPlan};
use super::super::present_target_stats::PresentTargetTracker;
use crate::config::{DepthReductionMode, PresentTargetConfig, PresentTimelinePolicy};

const PERIOD_NANOS: i64 = 16_666_667;
const PERIOD: Duration = Duration::from_nanos(16_666_667);
const START: i64 = 1_000_000_000;
/// Стоимость кадра после замеров: ближняя линия (срок через 12,67 мс) выполнима.
const MEASURED_COST: Duration = Duration::from_millis(1);

/// Момент vsync тика `tick`.
fn tick_at(tick: u64) -> i64 {
    START + i64::try_from(tick).expect("тик") * PERIOD_NANOS
}

/// Линии тика: линия `i` показывается через `i + 1` периодов, срок — за 4 мс до показа.
fn timelines_at(tick: u64) -> FrameTimelines {
    let vsync = tick_at(tick);
    FrameTimelines::from_platform_list(4, 1, |index| {
        let periods = i64::try_from(index).expect("индекс") + 1;
        VsyncTimeline {
            vsync_id: vsync / PERIOD_NANOS + periods,
            expected_present: MonotonicNanos::from_nanos(vsync + periods * PERIOD_NANOS),
            deadline: MonotonicNanos::from_nanos(vsync + periods * PERIOD_NANOS - 4_000_000),
        }
    })
    .expect("линии корректны")
}

/// Начальная оценка `initial_cost_us` уводит первый кадр на дальнюю линию; дальше замеры дешёвые.
fn config(initial_cost_us: u64, mode: DepthReductionMode, streak: u32, interval_ms: u64) -> PresentTargetConfig {
    PresentTargetConfig {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 },
        initial_frame_cost_us: initial_cost_us,
        gpu_cost_fallback_us: 0,
        min_cost_samples: 1,
        cost_window_frames: 4,
        depth_reduction: mode,
        depth_reduction_streak_frames: streak,
        depth_reduction_min_interval_ms: interval_ms,
        ..PresentTargetConfig::default()
    }
}

/// Что вышло на тике: глубина «показ − vsync тика» в периодах или пропуск.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Кадр метит на vsync через столько периодов после тика.
    Frame {
        periods_ahead: i64,
        expected: i64,
    },
    Skip,
}

/// Гоняет `ticks` тиков; после первого кадра стоимость кадра падает до `MEASURED_COST`.
fn run(planner: &mut PresentTargetPlanner, ticks: u64) -> Vec<Step> {
    (0..ticks)
        .map(|tick| {
            let now = MonotonicNanos::from_nanos(tick_at(tick));
            let step = match planner.plan_tick(&timelines_at(tick), Some(now), PERIOD) {
                TickPlan::Present { target, .. } => Step::Frame {
                    periods_ahead: (target.expected_present.as_nanos() - tick_at(tick) + PERIOD_NANOS / 2)
                        / PERIOD_NANOS,
                    expected: target.expected_present.as_nanos(),
                },
                TickPlan::SkipForDepthReduction => Step::Skip,
            };
            planner.record_frame_cost(MEASURED_COST);
            step
        })
        .collect()
}

/// Никакие два кадра не метят на один vsync, и показы идут вперёд.
fn assert_distinct_vsyncs(steps: &[Step]) {
    let expected: Vec<i64> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Frame { expected, .. } => Some(*expected),
            Step::Skip => None,
        })
        .collect();
    assert!(
        expected.windows(2).all(|pair| pair[1] - pair[0] > PERIOD_NANOS / 2),
        "два кадра на один vsync: {steps:?}"
    );
}

fn skip_positions(steps: &[Step]) -> Vec<usize> {
    steps
        .iter()
        .enumerate()
        .filter(|(_, step)| **step == Step::Skip)
        .map(|(index, _)| index)
        .collect()
}

fn periods_ahead(step: Step) -> Option<i64> {
    match step {
        Step::Frame { periods_ahead, .. } => Some(periods_ahead),
        Step::Skip => None,
    }
}

#[test]
fn a_steadily_feasible_nearer_timeline_is_reached_by_skipping_exactly_one_tick() {
    // Первый кадр: 15 + 1 мс > 12,67 мс до срока ближней линии — метит на следующую (2 периода).
    let mut planner = PresentTargetPlanner::new(config(15_000, DepthReductionMode::SkipOneVsync, 5, 0));
    let steps = run(&mut planner, 30);
    assert_distinct_vsyncs(&steps);
    assert_eq!(
        skip_positions(&steps),
        vec![6],
        "кадр 0 + серия 5 кадров, затем один пропуск"
    );
    assert!(
        steps[..6].iter().all(|step| periods_ahead(*step) == Some(2)),
        "{steps:?}"
    );
    assert!(
        steps[7..].iter().all(|step| periods_ahead(*step) == Some(1)),
        "после пропуска — ближняя линия навсегда: {steps:?}"
    );
}

#[test]
fn without_depth_reduction_the_planner_stays_on_the_deep_timeline_forever() {
    // Поведение, найденное стендом P13: запрет «строго позже прошлой линии» держит глубину.
    let mut planner = PresentTargetPlanner::new(config(15_000, DepthReductionMode::Disabled, 5, 0));
    let steps = run(&mut planner, 30);
    assert!(skip_positions(&steps).is_empty());
    assert!(steps.iter().all(|step| periods_ahead(*step) == Some(2)), "{steps:?}");
}

#[test]
fn the_preferred_policy_never_skips() {
    let mut planner = PresentTargetPlanner::new(PresentTargetConfig {
        policy: PresentTimelinePolicy::Preferred,
        ..config(15_000, DepthReductionMode::SkipOneVsync, 1, 0)
    });
    let steps = run(&mut planner, 30);
    assert!(skip_positions(&steps).is_empty());
}

#[test]
fn skips_are_spaced_by_the_minimum_interval_and_each_removes_one_level() {
    // 30 + 1 мс: первый кадр на 3 периода вперёд. Пауза 500 мс = 30 тиков.
    let mut planner = PresentTargetPlanner::new(config(30_000, DepthReductionMode::SkipOneVsync, 5, 500));
    let steps = run(&mut planner, 80);
    assert_distinct_vsyncs(&steps);
    let skips = skip_positions(&steps);
    assert_eq!(skips.len(), 2, "две ступени — два пропуска: {steps:?}");
    assert_eq!(skips[0], 6);
    assert!(skips[1] - skips[0] >= 30, "пропуски не чаще паузы: {skips:?}");
    assert_eq!(
        periods_ahead(steps[skips[0] + 1]),
        Some(2),
        "первый сход — на одну ступень"
    );
    assert_eq!(periods_ahead(steps[skips[1] + 1]), Some(1));
    assert!(steps[skips[1] + 1..].iter().all(|step| periods_ahead(*step) == Some(1)));
}

#[test]
fn an_unreadable_clock_breaks_the_streak() {
    let mut planner = PresentTargetPlanner::new(config(15_000, DepthReductionMode::SkipOneVsync, 5, 0));
    let mut steps = run(&mut planner, 4); // кадр 0 + серия 3
    // Часы не прочитались: откат на предпочитаемую, серия обнулена.
    assert!(matches!(
        planner.plan_tick(&timelines_at(4), None, PERIOD),
        TickPlan::Present { .. }
    ));
    steps.push(Step::Frame {
        periods_ahead: 2,
        expected: 0,
    });
    for tick in 5..20 {
        let now = MonotonicNanos::from_nanos(tick_at(tick));
        let skipped = planner.plan_tick(&timelines_at(tick), Some(now), PERIOD) == TickPlan::SkipForDepthReduction;
        steps.push(if skipped {
            Step::Skip
        } else {
            Step::Frame {
                periods_ahead: 0,
                expected: 0,
            }
        });
    }
    // После обнуления нужна полная серия 5 кадров заново: тики 5..=9, пропуск на 10-м.
    assert_eq!(skip_positions(&steps), vec![10]);
}

#[test]
fn the_tracker_counts_skips_and_confirmed_reductions_and_spends_no_frame_index_on_a_skip() {
    let mut tracker = PresentTargetTracker::new(&config(15_000, DepthReductionMode::SkipOneVsync, 5, 0), 16);
    let mut frame_index = 0;
    for tick in 0..20 {
        let now = MonotonicNanos::from_nanos(tick_at(tick));
        if let TickPlan::Present { .. } = tracker.plan_frame(frame_index, &timelines_at(tick), Some(now), PERIOD) {
            frame_index += 1;
        }
        tracker.record_cpu_timing(crate::backend::FrameCpuTiming {
            acquire_wait: Duration::ZERO,
            latch_wait: Duration::ZERO,
            latch_result: crate::backend::LatchWaitResult::NotRequested,
            latch_to_submit: Some(MEASURED_COST),
        });
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.skipped_ticks_for_depth, 1);
    assert_eq!(snapshot.depth_reductions, 1);
    assert_eq!(snapshot.depth_skips_without_reduction, 0);
    assert_eq!(snapshot.frames_planned, 19, "пропущенный тик — не кадр");
    assert_eq!(frame_index, 19);
}

#[test]
fn a_skip_after_which_the_nearer_timeline_became_infeasible_is_not_counted_as_a_reduction() {
    use super::DepthOutcome;

    let mut planner = PresentTargetPlanner::new(config(15_000, DepthReductionMode::SkipOneVsync, 5, 0));
    let steps = run(&mut planner, 7);
    assert_eq!(skip_positions(&steps), vec![6]);
    // За пропущенный тик оценка стоимости выросла: ближняя линия снова невыполнима.
    planner.record_frame_cost(Duration::from_millis(15));
    let after_skip = planner.plan_tick(&timelines_at(7), Some(MonotonicNanos::from_nanos(tick_at(7))), PERIOD);
    let TickPlan::Present { target, depth_outcome } = after_skip else {
        panic!("после пропуска — кадр");
    };
    assert_eq!(target.timeline_index, 1, "та же глубина, что до пропуска");
    assert_eq!(depth_outcome, Some(DepthOutcome::NotReduced));

    // И наоборот: при дешёвом кадре первый кадр после пропуска — `Reduced`.
    let mut planner = PresentTargetPlanner::new(config(15_000, DepthReductionMode::SkipOneVsync, 5, 0));
    run(&mut planner, 7);
    let after_skip = planner.plan_tick(&timelines_at(7), Some(MonotonicNanos::from_nanos(tick_at(7))), PERIOD);
    assert!(matches!(
        after_skip,
        TickPlan::Present {
            depth_outcome: Some(DepthOutcome::Reduced),
            ..
        }
    ));
}
