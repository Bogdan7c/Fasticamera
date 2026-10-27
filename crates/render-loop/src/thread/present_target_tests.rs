//! Тесты выбора линии кадра: самая ранняя выполнимая, откат, запас, оценка стоимости,
//! момент позднего захвата и защита от двух кадров на один vsync.

use shell_contract::vsync::VsyncTimeline;

use super::*;

/// Период 60 Гц, нс.
const PERIOD_NANOS: i64 = 16_666_667;
const PERIOD: Duration = Duration::from_nanos(16_666_667);
/// «Сейчас» в тестах.
const NOW: i64 = 1_000_000_000;

/// Линии как у Choreographer: линия `i` показывается через `(i + 1)` периодов, срок — за
/// 4 мс до показа. Предпочитаемая — `preferred`.
fn timelines(count: usize, preferred: usize) -> FrameTimelines {
    FrameTimelines::from_platform_list(count, preferred, |index| {
        let periods = i64::try_from(index).expect("индекс") + 1;
        VsyncTimeline {
            vsync_id: 100 + periods,
            expected_present: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS),
            deadline: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS - 4_000_000),
        }
    })
    .expect("линии корректны")
}

fn config(policy: PresentTimelinePolicy) -> PresentTargetConfig {
    PresentTargetConfig {
        policy,
        initial_frame_cost_us: 3_000,
        // Слагаемое GPU до замеров выключено: здесь проверяется CPU-часть оценки (GPU — отдельные тесты).
        gpu_cost_fallback_us: 0,
        min_cost_samples: 4,
        cost_window_frames: 8,
        latch_margin_us: 1_000,
        ..PresentTargetConfig::default()
    }
}

fn nearest(margin_us: u64) -> PresentTimelinePolicy {
    PresentTimelinePolicy::NearestFeasible { margin_us }
}

fn now() -> Option<MonotonicNanos> {
    Some(MonotonicNanos::from_nanos(NOW))
}

#[test]
fn preferred_policy_takes_the_platform_choice_even_if_an_earlier_one_fits() {
    let mut planner = PresentTargetPlanner::new(config(PresentTimelinePolicy::Preferred));
    let target = planner.plan(&timelines(4, 2), now(), PERIOD);
    assert_eq!(target.timeline_index, 2);
    assert_eq!(target.choice, TargetChoice::PlatformPreferred);
    assert_eq!(target.expected_present.as_nanos(), NOW + 3 * PERIOD_NANOS);
}

#[test]
fn nearest_feasible_picks_the_earliest_timeline_whose_deadline_fits_cost_plus_margin() {
    // Линия 0: срок через 16,67 − 4 = 12,67 мс; стоимость 3 + запас 2 = 5 мс — успеваем.
    let mut planner = PresentTargetPlanner::new(config(nearest(2_000)));
    let target = planner.plan(&timelines(4, 2), now(), PERIOD);
    assert_eq!(target.timeline_index, 0);
    assert_eq!(target.preferred_index, 2);
    assert_eq!(target.choice, TargetChoice::NearestFeasible);
    assert_eq!(target.vsync_id, 101);
}

#[test]
fn margin_that_does_not_fit_the_first_deadline_moves_to_the_next_timeline() {
    // Линия 0: до срока 12,67 мс < 3 + 10 мс; линия 1: 29,3 мс — успеваем.
    let mut planner = PresentTargetPlanner::new(config(nearest(10_000)));
    let target = planner.plan(&timelines(4, 3), now(), PERIOD);
    assert_eq!(target.timeline_index, 1);
    assert_eq!(target.choice, TargetChoice::NearestFeasible);
}

#[test]
fn deadline_exactly_at_cost_plus_margin_is_feasible_one_nanosecond_less_is_not() {
    // Срок линии 0 ровно в «сейчас + стоимость + запас»: граница включается.
    let exact = FrameTimelines::only(VsyncTimeline {
        vsync_id: 1,
        expected_present: MonotonicNanos::from_nanos(NOW + 10_000_000),
        deadline: MonotonicNanos::from_nanos(NOW + 5_000_000),
    });
    let rule = |lead_nanos: u64| FeasibilityRule {
        now: MonotonicNanos::from_nanos(NOW),
        required_lead: Duration::from_nanos(lead_nanos),
        not_before: None,
    };
    assert_eq!(nearest_feasible_timeline(&exact, rule(5_000_000)), Some(0));
    assert_eq!(nearest_feasible_timeline(&exact, rule(5_000_001)), None);
}

#[test]
fn no_feasible_timeline_falls_back_to_preferred() {
    // Запас 100 мс: ни одна из трёх линий (сроки через 12,7 / 29,3 / 46 мс) не выполнима.
    let mut planner = PresentTargetPlanner::new(config(nearest(100_000)));
    let target = planner.plan(&timelines(3, 1), now(), PERIOD);
    assert_eq!(target.timeline_index, 1);
    assert_eq!(target.choice, TargetChoice::FallbackToPreferred);
}

#[test]
fn unreadable_clock_falls_back_to_preferred() {
    let mut planner = PresentTargetPlanner::new(config(nearest(1_000)));
    let target = planner.plan(&timelines(3, 2), None, PERIOD);
    assert_eq!(target.timeline_index, 2);
    assert_eq!(target.choice, TargetChoice::FallbackToPreferred);
}

#[test]
fn latch_is_deadline_minus_estimated_cost_minus_margin() {
    let mut planner = PresentTargetPlanner::new(config(nearest(2_000)));
    let target = planner.plan(&timelines(3, 2), now(), PERIOD);
    // Линия 0: срок NOW + 12,666667 мс; стоимость 3 мс (начальная), запас 2 мс.
    assert_eq!(target.deadline.as_nanos(), NOW + PERIOD_NANOS - 4_000_000);
    assert_eq!(
        target.latch.latch_at.as_nanos(),
        NOW + PERIOD_NANOS - 4_000_000 - 5_000_000
    );
    assert_eq!(target.latch.spin_window, Duration::from_micros(500));
    // Конфиг разрешает 50 мс, но дольше периода без окна активного ожидания ждать нельзя.
    assert_eq!(target.latch.max_wait, PERIOD - Duration::from_micros(500));

    let mut preferred = PresentTargetPlanner::new(config(PresentTimelinePolicy::Preferred));
    let target = preferred.plan(&timelines(3, 2), now(), PERIOD);
    // Для Preferred запас — latch_margin_us (1 мс).
    assert_eq!(target.latch.latch_at.as_nanos(), target.deadline.as_nanos() - 4_000_000);
}

#[test]
fn estimated_cost_switches_from_initial_to_p95_after_enough_samples() {
    let mut planner = PresentTargetPlanner::new(config(PresentTimelinePolicy::Preferred));
    for _ in 0..3 {
        planner.record_frame_cost(Duration::from_micros(500));
    }
    assert_eq!(
        planner.estimated_frame_cost().total(),
        Duration::from_millis(3),
        "3 замера < 4: начальная оценка"
    );
    planner.record_frame_cost(Duration::from_micros(700));
    assert_eq!(
        planner.estimated_frame_cost().total(),
        Duration::from_micros(700),
        "p95 из четырёх — максимум"
    );
    // Окно 8: старые замеры вытесняются, оценка следует за нагрузкой.
    for _ in 0..8 {
        planner.record_frame_cost(Duration::from_micros(400));
    }
    assert_eq!(planner.estimated_frame_cost().total(), Duration::from_micros(400));
}

#[test]
fn measured_cost_moves_the_choice_to_a_later_timeline() {
    let mut planner = PresentTargetPlanner::new(config(nearest(1_000)));
    // Стоимость кадра 14 мс: линия 0 (12,67 мс до срока) уже не выполнима.
    for _ in 0..4 {
        planner.record_frame_cost(Duration::from_millis(14));
    }
    let target = planner.plan(&timelines(3, 2), now(), PERIOD);
    assert_eq!(target.timeline_index, 1);
}

#[test]
fn next_frame_never_targets_the_vsync_already_taken_by_the_previous_frame() {
    let mut planner = PresentTargetPlanner::new(config(nearest(1_000)));
    // Прошлый кадр взял линию 1 этого же набора (NOW + 2 периода): выброс стоимости.
    for _ in 0..4 {
        planner.record_frame_cost(Duration::from_millis(14));
    }
    assert_eq!(planner.plan(&timelines(3, 2), now(), PERIOD).timeline_index, 1);
    // Стоимость снова мала, следующий тик на период позже: его линия 0 — тот же vsync (NOW + 2 периода).
    for _ in 0..8 {
        planner.record_frame_cost(Duration::from_micros(500));
    }
    let later = FrameTimelines::from_platform_list(3, 2, |index| {
        let periods = i64::try_from(index).expect("индекс") + 2;
        VsyncTimeline {
            vsync_id: 100 + periods,
            expected_present: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS + 50_000),
            deadline: MonotonicNanos::from_nanos(NOW + periods * PERIOD_NANOS - 4_000_000),
        }
    })
    .expect("линии");
    let target = planner.plan(&later, Some(MonotonicNanos::from_nanos(NOW + PERIOD_NANOS)), PERIOD);
    assert_eq!(
        target.timeline_index, 1,
        "vsync прошлого кадра пропущен, взят следующий"
    );
    assert_eq!(target.vsync_id, 103);
}

// ---- предел ожидания позднего захвата и период тика ----

/// Часы теста: время стоит на месте, любой сон или активное ожидание записываются.
struct FrozenTimer {
    now_nanos: i64,
    waited: std::cell::Cell<bool>,
}

impl crate::thread::monotonic_clock::MonotonicTimer for FrozenTimer {
    fn now(&self) -> Result<MonotonicNanos, crate::thread::monotonic_clock::ClockReadError> {
        Ok(MonotonicNanos::from_nanos(self.now_nanos))
    }

    fn sleep(&self, _duration: Duration) {
        self.waited.set(true);
    }

    fn spin_once(&self) {
        self.waited.set(true);
    }
}

/// Линии 120 Гц: линия `i` показывается через `(i + 1)` периодов 120 Гц от `base`.
fn timelines_120_hz(base: i64, count: usize, preferred: usize) -> FrameTimelines {
    FrameTimelines::from_platform_list(count, preferred, |index| {
        let periods = i64::try_from(index).expect("индекс") + 1;
        VsyncTimeline {
            vsync_id: 200 + periods,
            expected_present: MonotonicNanos::from_nanos(base + periods * PERIOD_NANOS / 2),
            deadline: MonotonicNanos::from_nanos(base + periods * PERIOD_NANOS / 2 - 2_000_000),
        }
    })
    .expect("линии")
}

/// `Preferred` метит на 3-й vsync: `latch_at` через ~36 мс. Ждать его с картинкой swapchain на
/// руках — проспать два vsync; ожидание обязано быть отклонено, а не растянуто на 50 мс конфига.
#[test]
fn latch_wait_never_exceeds_one_vsync_period_minus_the_spin_window() {
    let mut planner = PresentTargetPlanner::new(PresentTargetConfig {
        late_latch: crate::config::LateLatchMode::BeforeCameraPoll,
        ..config(PresentTimelinePolicy::Preferred)
    });
    let target = planner.plan(&timelines(4, 2), now(), PERIOD);
    assert_eq!(target.latch.max_wait, PERIOD - Duration::from_micros(500));
    let timer = FrozenTimer {
        now_nanos: NOW,
        waited: std::cell::Cell::new(false),
    };
    let outcome = super::super::latch_sleep::sleep_until(
        super::super::latch_sleep::SleepUntil {
            target: target.latch.latch_at,
            spin_window: target.latch.spin_window,
            max_wait: target.latch.max_wait,
        },
        &timer,
    );
    assert!(
        matches!(
            outcome,
            super::super::latch_sleep::SleepUntilOutcome::RefusedTooFar { .. }
        ),
        "{outcome:?}"
    );
    assert!(!timer.waited.get(), "поток не спал с картинкой swapchain на руках");
}

/// Сразу после 60 → 120 Гц пейсинг ещё сообщает 16,7 мс: предел ожидания берётся по шагу линий
/// тика (8,3 мс), а не по устаревшему периоду.
#[test]
fn latch_wait_budget_follows_the_tick_spacing_after_a_refresh_rate_switch() {
    let mut planner = PresentTargetPlanner::new(config(nearest(1_000)));
    let target = planner.plan(&timelines_120_hz(NOW, 3, 1), now(), PERIOD);
    assert_eq!(
        target.latch.max_wait,
        Duration::from_nanos(u64::try_from(PERIOD_NANOS / 2).expect("период")) - Duration::from_micros(500)
    );
    assert_eq!(
        tick_vsync_period(&timelines(3, 1), PERIOD / 2),
        PERIOD / 2,
        "пейсинг короче шага линий — берётся он"
    );
}

/// После 60 → 120 Гц: прошлый кадр взял vsync T, настоящий следующий — T + 8,3 мс. Со старым
/// периодом пейсинга (16,7 мс) порог «прошлая + полпериода» = T + 8,3 мс и строгое `>` выкинул бы
/// этот vsync; по шагу линий тика порог T + 4,2 мс — vsync берётся.
#[test]
fn distinct_vsync_floor_uses_the_tick_spacing_right_after_a_switch_to_120_hz() {
    let mut planner = PresentTargetPlanner::new(config(nearest(1_000)));
    let first = planner.plan(&timelines_120_hz(NOW, 3, 1), now(), PERIOD);
    assert_eq!(first.timeline_index, 0);
    let next_tick_now = NOW + PERIOD_NANOS / 2;
    let second = planner.plan(
        &timelines_120_hz(next_tick_now, 3, 1),
        Some(MonotonicNanos::from_nanos(next_tick_now)),
        PERIOD,
    );
    assert_eq!(second.timeline_index, 0, "ближайший новый vsync не выкинут");
    assert!(second.expected_present > first.expected_present);
}

// ---- Слагаемое GPU в стоимости кадра (P13 сессии 07) ----

/// Планировщик с заданной оценкой GPU до замеров и порогом замеров 4.
fn planner_with_gpu_fallback(policy: PresentTimelinePolicy, gpu_fallback_us: u64) -> PresentTargetPlanner {
    PresentTargetPlanner::new(PresentTargetConfig {
        gpu_cost_fallback_us: gpu_fallback_us,
        ..config(policy)
    })
}

#[test]
fn gpu_part_uses_the_configured_fallback_until_enough_gpu_samples_then_their_p95() {
    let mut planner = planner_with_gpu_fallback(PresentTimelinePolicy::Preferred, 13_000);
    let before = planner.estimated_frame_cost();
    assert_eq!(before.gpu, Duration::from_millis(13));
    assert_eq!(before.gpu_source, GpuCostSource::ConfiguredFallback);
    assert_eq!(
        before.total(),
        Duration::from_millis(16),
        "CPU 3 мс (начальная) + GPU 13 мс"
    );
    for gpu_ms in [5, 6, 7] {
        planner.record_gpu_cost(Duration::from_millis(gpu_ms));
    }
    assert_eq!(
        planner.estimated_frame_cost().gpu_source,
        GpuCostSource::ConfiguredFallback,
        "3 замера < 4: ещё оценка из конфига"
    );
    planner.record_gpu_cost(Duration::from_millis(9));
    let measured = planner.estimated_frame_cost();
    assert_eq!(measured.gpu_source, GpuCostSource::Measured);
    assert_eq!(measured.gpu, Duration::from_millis(9), "p95 из четырёх — максимум");
    assert_eq!(
        measured.cpu,
        Duration::from_millis(3),
        "CPU-часть не тронута замерами GPU"
    );
}

#[test]
fn gpu_time_moves_latch_at_earlier_by_exactly_that_time() {
    let timelines = timelines(4, 2);
    let mut cpu_only = planner_with_gpu_fallback(PresentTimelinePolicy::Preferred, 0);
    let mut with_gpu = planner_with_gpu_fallback(PresentTimelinePolicy::Preferred, 0);
    for _ in 0..4 {
        with_gpu.record_gpu_cost(Duration::from_micros(7_250));
    }
    let base = cpu_only.plan(&timelines, now(), PERIOD);
    let shifted = with_gpu.plan(&timelines, now(), PERIOD);
    assert_eq!(base.timeline_index, shifted.timeline_index, "Preferred: линия та же");
    assert_eq!(
        base.latch.latch_at.as_nanos() - shifted.latch.latch_at.as_nanos(),
        7_250_000,
        "latch_at сдвинулся раньше ровно на время GPU"
    );
}

#[test]
fn gpu_time_moves_the_nearest_feasible_choice_to_a_later_timeline_and_back() {
    // Линия 0: до срока 12,67 мс. CPU 3 + запас 1 = 4 мс — успеваем без GPU.
    let timelines = timelines(4, 3);
    let mut planner = planner_with_gpu_fallback(nearest(1_000), 0);
    assert_eq!(planner.plan(&timelines, now(), PERIOD).timeline_index, 0);
    // GPU 10 мс: 3 + 10 + 1 = 14 > 12,67 — линия 0 не выполнима, берётся линия 1.
    let mut slow_gpu = planner_with_gpu_fallback(nearest(1_000), 0);
    for _ in 0..4 {
        slow_gpu.record_gpu_cost(Duration::from_millis(10));
    }
    let later = slow_gpu.plan(&timelines, now(), PERIOD);
    assert_eq!(later.timeline_index, 1);
    assert_eq!(later.choice, TargetChoice::NearestFeasible);
    // Окно 8: восемь быстрых кадров GPU вытесняют медленные — выбор возвращается к линии 0.
    let mut recovering = planner_with_gpu_fallback(nearest(1_000), 0);
    for gpu_ms in [10, 10, 10, 10, 2, 2, 2, 2, 2, 2, 2, 2] {
        recovering.record_gpu_cost(Duration::from_millis(gpu_ms));
    }
    assert_eq!(recovering.plan(&timelines, now(), PERIOD).timeline_index, 0);
}

#[test]
fn configured_gpu_fallback_alone_moves_the_choice_when_gpu_is_not_measured() {
    // Бэкенд GPU не меряет: консервативная оценка 13 мс делает линию 0 невыполнимой.
    let timelines = timelines(4, 3);
    let mut planner = planner_with_gpu_fallback(nearest(1_000), 13_000);
    let target = planner.plan(&timelines, now(), PERIOD);
    assert_eq!(target.timeline_index, 1);
    assert_eq!(
        target.deadline.as_nanos() - target.latch.latch_at.as_nanos(),
        17_000_000,
        "latch_at = срок − (CPU 3 + GPU 13 + запас 1) мс"
    );
}

#[test]
fn reconfigure_forgets_gpu_samples_of_the_previous_phase() {
    let mut planner = planner_with_gpu_fallback(PresentTimelinePolicy::Preferred, 13_000);
    for _ in 0..4 {
        planner.record_gpu_cost(Duration::from_millis(2));
    }
    assert_eq!(planner.estimated_frame_cost().gpu_source, GpuCostSource::Measured);
    planner.reconfigure(PresentTargetConfig {
        gpu_cost_fallback_us: 13_000,
        ..config(nearest(1_000))
    });
    let reset = planner.estimated_frame_cost();
    assert_eq!(reset.gpu_source, GpuCostSource::ConfiguredFallback);
    assert_eq!(reset.gpu, Duration::from_millis(13));
}
