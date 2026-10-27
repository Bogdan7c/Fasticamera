//! Тесты учёта выбора линии: гистограммы выбора и опоздания по синтетическим показам,
//! откаты, отброшенные линии и окна замеров CPU.

use shell_contract::vsync::VsyncTimeline;

use super::super::present_target::GpuCostSource;
use super::*;
use crate::config::PresentTimelinePolicy;

const PERIOD_NANOS: i64 = 16_666_667;
const PERIOD: Duration = Duration::from_nanos(16_666_667);
const NOW: i64 = 5_000_000_000;

fn timelines(count: usize, preferred: usize) -> FrameTimelines {
    FrameTimelines::from_platform_list(count, preferred, |index| {
        let periods = i64::try_from(index).expect("индекс") + 1;
        VsyncTimeline {
            vsync_id: periods,
            expected_present: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS),
            deadline: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS - 4_000_000),
        }
    })
    .expect("линии")
}

fn tracker(policy: PresentTimelinePolicy) -> PresentTargetTracker {
    let config = PresentTargetConfig {
        policy,
        initial_frame_cost_us: 2_000,
        // Слагаемое GPU до замеров выключено: здесь проверяется CPU-часть оценки (GPU — отдельные тесты).
        gpu_cost_fallback_us: 0,
        stats_window_frames: 10,
        // Поздний захват выключен, как в продукте до P14: тесты учёта сравнивают с ним команды.
        late_latch: crate::config::LateLatchMode::Disabled,
        ..PresentTargetConfig::default()
    };
    PresentTargetTracker::new(&config, 16)
}

fn now() -> Option<MonotonicNanos> {
    Some(MonotonicNanos::from_nanos(NOW))
}

fn shown(frame_index: u64, actual_nanos: i64) -> PastPresentation {
    PastPresentation {
        present_id: u32::try_from(frame_index).expect("номер"),
        frame_index,
        actual_present: MonotonicNanos::from_nanos(actual_nanos),
    }
}

#[test]
fn chosen_timeline_histogram_and_relation_to_preferred_are_counted() {
    let mut nearest = tracker(PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 });
    // Каждый кадр — новый тик на период позже, чтобы защита «два кадра на один vsync» не мешала.
    for frame in 0..3i64 {
        let shifted = FrameTimelines::from_platform_list(3, 2, |index| {
            let periods = i64::try_from(index).expect("индекс") + 1 + frame;
            VsyncTimeline {
                vsync_id: periods,
                expected_present: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS),
                deadline: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS - 4_000_000),
            }
        })
        .expect("линии");
        let tick_now = Some(MonotonicNanos::from_nanos(NOW + frame * PERIOD_NANOS));
        nearest.plan_frame(0, &shifted, tick_now, PERIOD);
    }
    let snapshot = nearest.snapshot();
    assert_eq!(snapshot.frames_planned, 3);
    assert_eq!(snapshot.chosen_timeline[0], 3, "каждый раз самая ранняя");
    assert_eq!(snapshot.chosen_earlier_than_preferred, 3);
    assert_eq!(snapshot.fallbacks_to_preferred, 0);

    let mut preferred = tracker(PresentTimelinePolicy::Preferred);
    preferred.plan_frame(0, &timelines(3, 2), now(), PERIOD);
    let snapshot = preferred.snapshot();
    assert_eq!(snapshot.chosen_timeline[2], 1);
    assert_eq!(snapshot.chosen_earlier_than_preferred, 0);
}

#[test]
fn fallback_and_clock_failure_are_counted_separately() {
    let mut nearest = tracker(PresentTimelinePolicy::NearestFeasible { margin_us: 500_000 });
    nearest.plan_frame(0, &timelines(3, 1), now(), PERIOD);
    nearest.plan_frame(0, &timelines(3, 1), None, PERIOD);
    let snapshot = nearest.snapshot();
    assert_eq!(snapshot.fallbacks_to_preferred, 2);
    assert_eq!(snapshot.clock_read_failures, 1);
    assert_eq!(snapshot.chosen_timeline[1], 2);
    // Запас 500 мс: момент захвата давно в прошлом.
    assert_eq!(snapshot.latch_already_passed, 1, "без часов «уже прошёл» не считается");
}

#[test]
fn ticks_with_truncated_timelines_are_counted() {
    let mut preferred = tracker(PresentTimelinePolicy::Preferred);
    preferred.plan_frame(0, &timelines(MAX_FRAME_TIMELINES + 2, 0), now(), PERIOD);
    preferred.plan_frame(0, &timelines(3, 0), now(), PERIOD);
    let snapshot = preferred.snapshot();
    assert_eq!(snapshot.ticks_with_dropped_timelines, 1);
    assert_eq!(snapshot.dropped_timelines, 2);
}

#[test]
fn vsync_shift_histogram_from_synthetic_presentations() {
    let mut preferred = tracker(PresentTimelinePolicy::Preferred);
    // Кадры 0..=4 метят каждый на NOW + 1 период (линия 0 одного и того же набора).
    let target = preferred.plan_frame(0, &timelines(1, 0), now(), PERIOD).expect_target();
    for frame_index in 0..5 {
        preferred.record_presented(frame_index, &target);
    }
    let expected = target.expected_present.as_nanos();
    preferred.record_presentation(&shown(0, expected + 200_000), PERIOD); // дрожание: на своём vsync
    preferred.record_presentation(&shown(1, expected - 300_000), PERIOD); // дрожание назад: тоже на своём
    preferred.record_presentation(&shown(2, expected + PERIOD_NANOS), PERIOD); // на 1 позже
    // Кадр 3: записи нет (потеряна драйвером) — не «unmatched» и не ломает сопоставление 4-го.
    preferred.record_presentation(&shown(4, expected + 5 * PERIOD_NANOS), PERIOD); // 5 позже -> «3+»
    preferred.record_presentation(&shown(9, expected), PERIOD); // неизвестный кадр
    let shift = preferred.snapshot().vsync_shift;
    assert_eq!(shift.late_by_periods, [2, 1, 0, 1]);
    assert_eq!(shift.unmatched, 1);
    assert_eq!(shift.early, 0);
}

#[test]
fn presentation_a_whole_period_before_target_is_early() {
    let mut preferred = tracker(PresentTimelinePolicy::Preferred);
    let target = preferred.plan_frame(0, &timelines(1, 0), now(), PERIOD).expect_target();
    preferred.record_presented(7, &target);
    preferred.record_presentation(&shown(7, target.expected_present.as_nanos() - PERIOD_NANOS), PERIOD);
    let shift = preferred.snapshot().vsync_shift;
    assert_eq!(shift.early, 1);
    assert_eq!(shift.late_by_periods, [0; VSYNC_SHIFT_BUCKETS]);
}

#[test]
fn cpu_timings_feed_the_windows_and_the_cost_estimate() {
    let mut preferred = tracker(PresentTimelinePolicy::Preferred);
    assert_eq!(preferred.snapshot().acquire_wait, None, "замеров нет");
    assert_eq!(
        preferred.snapshot().estimated_frame_cost,
        Duration::from_millis(2),
        "начальная оценка"
    );
    for step in 1..=40u64 {
        preferred.record_cpu_timing(FrameCpuTiming {
            acquire_wait: Duration::from_micros(step * 100),
            latch_wait: Duration::from_micros(250),
            latch_result: LatchWaitResult::Waited,
            latch_to_submit: Some(Duration::from_micros(900)),
        });
    }
    // Кадр, не дошедший до показа: acquire считается, стоимость — нет.
    preferred.record_cpu_timing(FrameCpuTiming {
        acquire_wait: Duration::from_millis(100),
        latch_wait: Duration::ZERO,
        latch_result: LatchWaitResult::Waited,
        latch_to_submit: None,
    });
    let snapshot = preferred.snapshot();
    let acquire = snapshot.acquire_wait.expect("замеры есть");
    assert_eq!(acquire.samples, 10, "окно stats_window_frames");
    assert_eq!(acquire.max, Duration::from_millis(100));
    assert_eq!(
        acquire.p50,
        Duration::from_micros(3_600),
        "окно — кадры 32..=40 и таймаут"
    );
    let latch_wait = snapshot.latch_wait.expect("замеры есть");
    assert_eq!(latch_wait.max, Duration::from_micros(250));
    assert_eq!(latch_wait.samples, 10);
    let cost = snapshot.latch_to_submit.expect("замеры есть");
    assert_eq!(cost.p95, Duration::from_micros(900));
    assert_eq!(
        snapshot.estimated_frame_cost,
        Duration::from_micros(900),
        "после 30 замеров — p95"
    );
}

#[test]
fn applied_command_switches_policy_and_starts_a_fresh_measurement_phase() {
    use crate::config::LateLatchMode;
    use crate::thread::present_target_control::{PresentTargetCommand, PresentTargetCommandOutcome};

    let mut tracker = tracker(PresentTimelinePolicy::Preferred);
    // Фаза A: предпочитаемая линия (индекс 2), замеры CPU, показ кадра 0 со сдвигом на 1 период.
    let first = tracker.plan_frame(0, &timelines(3, 2), now(), PERIOD).expect_target();
    assert_eq!(first.timeline_index, 2);
    tracker.record_cpu_timing(FrameCpuTiming {
        acquire_wait: Duration::from_micros(700),
        latch_wait: Duration::ZERO,
        latch_result: LatchWaitResult::Waited,
        latch_to_submit: Some(Duration::from_micros(900)),
    });
    tracker.record_presented(0, &first);
    let before = tracker.snapshot();
    assert_eq!(before.frames_planned, 1);
    assert_eq!(before.chosen_timeline[2], 1);

    let to_nearest = PresentTargetCommand::Set {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: crate::config::DepthReductionMode::default(),
    };
    assert_eq!(tracker.apply_command(to_nearest), PresentTargetCommandOutcome::Applied);
    let reset = tracker.snapshot();
    assert_eq!(reset.selection, to_nearest.selection(), "выбор виден в снимке");
    assert_eq!(reset.frames_planned, 0, "счётчики новой фазы начинаются с нуля");
    assert_eq!(reset.chosen_timeline, [0; MAX_FRAME_TIMELINES]);
    assert_eq!(reset.acquire_wait, None, "окна диагностики обнулены");
    assert_eq!(reset.latch_to_submit, None);
    assert_eq!(
        reset.estimated_frame_cost,
        Duration::from_micros(2_000),
        "оценка стоимости снова начальная"
    );

    // Запись о показе кадра прежней фазы приходит после команды — в новую гистограмму не идёт.
    tracker.record_presentation(&shown(0, NOW + 4 * PERIOD_NANOS), PERIOD);
    assert_eq!(tracker.snapshot().vsync_shift, VsyncShiftHistogram::default());

    // Новая фаза: ближайшая выполнимая (индекс 0), поздний захват из команды — в плане кадра.
    let second = tracker.plan_frame(0, &timelines(3, 2), now(), PERIOD).expect_target();
    assert_eq!(second.timeline_index, 0, "линия выбирается новой политикой");
    assert_eq!(second.latch.mode, LateLatchMode::BeforeCameraPoll);
    assert_eq!(tracker.snapshot().chosen_timeline[0], 1);

    // Повтор той же команды ничего не обнуляет.
    assert_eq!(
        tracker.apply_command(to_nearest),
        PresentTargetCommandOutcome::AlreadyInEffect
    );
    assert_eq!(tracker.snapshot().frames_planned, 1);
}

#[test]
fn invalid_command_is_rejected_and_changes_nothing() {
    use crate::config::{LateLatchMode, MAX_PRESENT_TARGET_DURATION_US};
    use crate::thread::present_target_control::{PresentTargetCommand, PresentTargetCommandOutcome};

    let mut tracker = tracker(PresentTimelinePolicy::Preferred);
    tracker.plan_frame(0, &timelines(3, 2), now(), PERIOD);
    let outcome = tracker.apply_command(PresentTargetCommand::Set {
        policy: PresentTimelinePolicy::NearestFeasible {
            margin_us: MAX_PRESENT_TARGET_DURATION_US + 1,
        },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: crate::config::DepthReductionMode::default(),
    });
    assert!(
        matches!(&outcome, PresentTargetCommandOutcome::Rejected { reason } if reason.contains("margin_us")),
        "{outcome:?}"
    );
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.selection.policy, PresentTimelinePolicy::Preferred);
    assert_eq!(snapshot.selection.late_latch, LateLatchMode::Disabled);
    assert_eq!(snapshot.frames_planned, 1, "отвергнутая команда фазу не обнуляет");
}

/// Отказы ждать момент захвата и урезание предела периодом видны в снимке.
#[test]
fn latch_refusals_and_period_caps_are_counted() {
    use crate::config::LateLatchMode;

    let config = PresentTargetConfig {
        policy: PresentTimelinePolicy::Preferred,
        late_latch: LateLatchMode::BeforeCameraPoll,
        ..PresentTargetConfig::default()
    };
    let mut tracker = PresentTargetTracker::new(&config, 16);
    let target = tracker.plan_frame(0, &timelines(3, 2), now(), PERIOD).expect_target();
    assert!(target.latch.max_wait < PERIOD, "{:?}", target.latch);
    tracker.record_cpu_timing(FrameCpuTiming {
        acquire_wait: Duration::from_micros(300),
        latch_wait: Duration::ZERO,
        latch_result: LatchWaitResult::RefusedBeyondBudget,
        latch_to_submit: Some(Duration::from_micros(900)),
    });
    tracker.record_cpu_timing(FrameCpuTiming {
        acquire_wait: Duration::from_micros(300),
        latch_wait: Duration::from_micros(200),
        latch_result: LatchWaitResult::Waited,
        latch_to_submit: Some(Duration::from_micros(900)),
    });
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.latch_wait_capped_to_period, 1);
    assert_eq!(snapshot.latch_waits_refused, 1, "считается только отказ");

    // Поздний захват выключен: урезание предела не считается (его никто не ждёт).
    let mut disabled = tracker_with_latch(LateLatchMode::Disabled);
    disabled.plan_frame(0, &timelines(3, 2), now(), PERIOD);
    assert_eq!(disabled.snapshot().latch_wait_capped_to_period, 0);
}

fn tracker_with_latch(late_latch: crate::config::LateLatchMode) -> PresentTargetTracker {
    let config = PresentTargetConfig {
        late_latch,
        ..PresentTargetConfig::default()
    };
    PresentTargetTracker::new(&config, 16)
}

#[test]
fn gpu_timings_feed_the_window_and_the_estimate_but_not_across_a_command() {
    let mut tracker = tracker(PresentTimelinePolicy::Preferred);
    for frame_index in 0..40 {
        tracker.plan_frame(frame_index, &timelines(3, 2), now(), PERIOD);
        tracker.record_gpu_timing(FrameGpuTiming {
            frame_index,
            gpu_duration: Duration::from_millis(8),
        });
    }
    let measured = tracker.snapshot();
    assert_eq!(measured.estimated_cost_parts.gpu_source, GpuCostSource::Measured);
    assert_eq!(measured.estimated_cost_parts.gpu, Duration::from_millis(8));
    assert_eq!(
        measured.estimated_frame_cost,
        Duration::from_millis(10),
        "CPU 2 (начальная) + GPU 8"
    );
    assert_eq!(
        measured.frame_gpu_time.map(|window| window.p95),
        Some(Duration::from_millis(8))
    );

    let command = PresentTargetCommand::Set {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_000 },
        late_latch: LateLatchMode::Disabled,
        depth_reduction: crate::config::DepthReductionMode::default(),
    };
    assert_eq!(tracker.apply_command(command), PresentTargetCommandOutcome::Applied);
    // Поздние замеры GPU кадров прежней фазы (номер ≤ 39) в новую фазу не идут.
    tracker.record_gpu_timing(FrameGpuTiming {
        frame_index: 39,
        gpu_duration: Duration::from_millis(30),
    });
    let reset = tracker.snapshot();
    assert_eq!(reset.frame_gpu_time, None);
    assert_eq!(reset.estimated_cost_parts.gpu_source, GpuCostSource::ConfiguredFallback);
    // Замер кадра новой фазы — идёт.
    tracker.plan_frame(40, &timelines(3, 2), now(), PERIOD);
    tracker.record_gpu_timing(FrameGpuTiming {
        frame_index: 40,
        gpu_duration: Duration::from_millis(3),
    });
    assert_eq!(tracker.snapshot().frame_gpu_time.map(|window| window.samples), Some(1));
}
