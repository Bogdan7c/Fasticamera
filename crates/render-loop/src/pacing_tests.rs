//! Тесты статистики пейсинга (вынесены из `pacing.rs`, чтобы модуль не раздувался).

use super::*;

/// 120 Гц: период 8 333 333 нс.
const PERIOD_NANOS: i64 = 8_333_333;

fn params(max_miss_ratio: f64, window_s: u64) -> PacingParams {
    PacingParams {
        refresh_period: Duration::from_nanos(PERIOD_NANOS as u64),
        max_miss_ratio,
        window: Duration::from_secs(window_s),
        period_change_ratio: 0.02,
        min_coverage_ratio: 0.9,
    }
}

/// Заливает метки показа: каждый интервал — `PERIOD_NANOS` + смещение из `deviation(i)`.
fn feed(tracker: &mut PacingTracker, frames: u32, deviation: impl Fn(u32) -> i64) {
    let mut now = 1_000_000_000i64;
    tracker.record_present(0, MonotonicNanos::from_nanos(now));
    for index in 0..frames {
        now += PERIOD_NANOS + deviation(index);
        tracker.record_present(index + 1, MonotonicNanos::from_nanos(now));
    }
}

#[test]
fn empty_tracker_has_not_enough_data() {
    let snapshot = PacingTracker::new(params(0.005, 10)).snapshot();
    assert_eq!(snapshot, PacingSnapshot::empty());
}

#[test]
fn single_present_has_no_intervals() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    tracker.record_present(0, MonotonicNanos::from_nanos(5));
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.intervals, 0);
    assert_eq!(snapshot.verdict, PacingVerdict::NotEnoughData);
}

#[test]
fn short_clean_run_is_not_enough_data() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    feed(&mut tracker, 120, |_| 0); // всего 1 секунда из 10
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0);
    assert_eq!(snapshot.verdict, PacingVerdict::NotEnoughData);
}

#[test]
fn perfect_120_hz_for_full_window_passes() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    feed(&mut tracker, 1300, |_| 0);
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0);
    assert_eq!(snapshot.histogram[1], snapshot.intervals, "все интервалы — один период");
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
}

#[test]
fn jitter_within_tolerance_is_not_a_miss() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    // +-3 мс вокруг 8,33 мс: интервал максимум 11,3 мс < 12,5 мс (1,5 периода).
    feed(
        &mut tracker,
        1300,
        |index| if index % 2 == 0 { 3_000_000 } else { -3_000_000 },
    );
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0);
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
}

#[test]
fn dropped_frames_beyond_threshold_fail_with_exact_counts() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    // Каждый 100-й интервал — два периода (пропущен один vsync): 1 % > 0,5 %.
    feed(
        &mut tracker,
        1300,
        |index| if index % 100 == 50 { PERIOD_NANOS } else { 0 },
    );
    let snapshot = tracker.snapshot();
    assert!(snapshot.misses >= 10, "пропусков: {}", snapshot.misses);
    assert_eq!(
        snapshot.histogram[2], snapshot.misses,
        "пропуск = корзина двух периодов"
    );
    match snapshot.verdict {
        PacingVerdict::Failed {
            misses,
            intervals,
            max_miss_ratio,
            ..
        } => {
            assert_eq!(misses, snapshot.misses);
            assert_eq!(intervals, snapshot.intervals);
            assert_eq!(max_miss_ratio, 0.005);
        }
        other => panic!("ожидали Failed, получили {other:?}"),
    }
}

#[test]
fn single_drop_in_long_window_stays_under_threshold() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    feed(&mut tracker, 1300, |index| if index == 600 { PERIOD_NANOS } else { 0 });
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 1);
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
}

#[test]
fn interval_exactly_one_and_a_half_periods_is_not_a_miss() {
    let mut tracker = PacingTracker::new(PacingParams {
        refresh_period: Duration::from_nanos(1000),
        max_miss_ratio: 0.0,
        window: Duration::from_nanos(1000),
        period_change_ratio: 0.02,
        min_coverage_ratio: 0.9,
    });
    tracker.record_present(0, MonotonicNanos::from_nanos(0));
    tracker.record_present(1, MonotonicNanos::from_nanos(1500));
    assert_eq!(
        tracker.snapshot().misses,
        0,
        "граница строгая: 1,5 периода — ещё не пропуск"
    );
    tracker.record_present(2, MonotonicNanos::from_nanos(3001));
    assert_eq!(tracker.snapshot().misses, 1);
}

#[test]
fn long_stall_lands_in_last_bucket() {
    let mut tracker = PacingTracker::new(params(0.5, 10));
    tracker.record_present(0, MonotonicNanos::from_nanos(0));
    tracker.record_present(1, MonotonicNanos::from_nanos(PERIOD_NANOS * 50));
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.histogram[HISTOGRAM_BUCKETS - 1], 1);
    assert_eq!(snapshot.misses, 1);
}

#[test]
fn old_presents_leave_the_window() {
    let mut tracker = PacingTracker::new(params(0.005, 1));
    // Секунда плохих кадров, затем 2 секунды идеальных: плохие вытеснены из окна в 1 с.
    let mut now = 0i64;
    let mut present_id = 0u32;
    tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    for _ in 0..120 {
        now += PERIOD_NANOS * 3;
        present_id += 1;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    }
    for _ in 0..240 {
        now += PERIOD_NANOS;
        present_id += 1;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0);
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
    assert!(snapshot.observed_span <= Duration::from_secs(1));
}

#[test]
fn out_of_order_timestamp_is_rejected_and_counted() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    tracker.record_present(0, MonotonicNanos::from_nanos(1_000));
    tracker.record_present(1, MonotonicNanos::from_nanos(500));
    assert_eq!(tracker.rejected_out_of_order(), 1);
    assert_eq!(tracker.snapshot().intervals, 0);
}
/// Экран переключил режим на ходу: интервалы старого периода не смешиваются с новыми.
/// Раньше окно переоценивалось новым периодом целиком, и 60-герцовые интервалы
/// выглядели бы «пропусками» на 120 Гц.
#[test]
fn period_change_clears_window_instead_of_mixing_intervals() {
    const INTERVAL_60_HZ_NANOS: i64 = 16_666_667;
    let mut tracker = PacingTracker::new(PacingParams {
        refresh_period: Duration::from_nanos(INTERVAL_60_HZ_NANOS as u64),
        max_miss_ratio: 0.005,
        window: Duration::from_secs(10),
        period_change_ratio: 0.02,
        min_coverage_ratio: 0.9,
    });
    let mut now = 1_000_000_000i64;
    for present_id in 0..60u32 {
        now += INTERVAL_60_HZ_NANOS;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    }
    assert_eq!(tracker.snapshot().intervals, 59);

    let new_period = Duration::from_nanos(PERIOD_NANOS as u64);
    assert!(tracker.set_refresh_period(new_period), "период изменился");
    assert_eq!(tracker.refresh_period(), new_period);
    assert_eq!(tracker.period_changes(), 1);
    assert_eq!(tracker.snapshot(), PacingSnapshot::empty(), "окно очищено");

    // Дальше идут 120-герцовые кадры: судятся только по новому периоду, без ложных пропусков.
    for present_id in 60..260u32 {
        now += PERIOD_NANOS;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.intervals, 199);
    assert_eq!(snapshot.misses, 0);
    assert_eq!(
        snapshot.histogram[1], snapshot.intervals,
        "все интервалы — один новый период"
    );
    assert_eq!(snapshot.miss_ratio, 0.0);
}

/// Потерянные записи (несоседние номера) — дыра измерения, а не пропуск кадра.
#[test]
fn lost_records_are_gaps_not_misses() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    let mut now = 0i64;
    // Идеальные 120 Гц, но драйвер потерял по 3 записи после каждой десятой: ids 0..9, 13..22, ...
    let mut present_id = 0u32;
    for frame in 0..400u32 {
        now += PERIOD_NANOS;
        if frame % 10 == 0 && frame > 0 {
            present_id += 3; // потеряно 3 записи
        }
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
        present_id += 1;
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0, "дыры не должны считаться пропусками");
    assert_eq!(snapshot.timing_gaps, 3 * 39, "39 разрывов по 3 потерянные записи");
    assert_eq!(
        snapshot.intervals,
        399 - 39,
        "интервалы только между соседними номерами"
    );
    assert_eq!(snapshot.frames_covered, 400 + 3 * 39);
    assert_eq!(snapshot.histogram[1], snapshot.intervals);
    assert_eq!(snapshot.miss_ratio, 0.0);
}

/// Настоящий пропуск между соседними номерами (2 периода) по-прежнему пропуск.
#[test]
fn two_period_interval_between_consecutive_ids_is_a_miss() {
    let mut tracker = PacingTracker::new(params(0.5, 10));
    tracker.record_present(7, MonotonicNanos::from_nanos(0));
    tracker.record_present(8, MonotonicNanos::from_nanos(PERIOD_NANOS * 2));
    let snapshot = tracker.snapshot();
    assert_eq!((snapshot.intervals, snapshot.misses, snapshot.timing_gaps), (1, 1, 0));
    assert_eq!(snapshot.histogram[2], 1);
}

/// Те же 2 периода между НЕсоседними номерами — это дыра, пропуска нет.
#[test]
fn two_period_distance_across_lost_record_is_not_a_miss() {
    let mut tracker = PacingTracker::new(params(0.5, 10));
    tracker.record_present(7, MonotonicNanos::from_nanos(0));
    tracker.record_present(9, MonotonicNanos::from_nanos(PERIOD_NANOS * 2));
    let snapshot = tracker.snapshot();
    assert_eq!((snapshot.intervals, snapshot.misses, snapshot.timing_gaps), (0, 0, 1));
}

/// Номера циклические: переход через `u32::MAX` остаётся соседним.
#[test]
fn ids_wrapping_around_u32_are_still_consecutive() {
    let mut tracker = PacingTracker::new(params(0.5, 10));
    tracker.record_present(u32::MAX, MonotonicNanos::from_nanos(0));
    tracker.record_present(0, MonotonicNanos::from_nanos(PERIOD_NANOS));
    let snapshot = tracker.snapshot();
    assert_eq!((snapshot.intervals, snapshot.timing_gaps), (1, 0));
}

/// Пересоздание цепочки: новые номера не продолжают старые, пара через разрыв
/// не даёт ни интервала, ни дыры (иначе разрыв стал бы ложным пропуском или дырой).
#[test]
fn discontinuity_breaks_continuity_without_interval_or_gap() {
    let mut tracker = PacingTracker::new(params(0.5, 10));
    tracker.record_present(100, MonotonicNanos::from_nanos(0));
    tracker.record_present(101, MonotonicNanos::from_nanos(PERIOD_NANOS));
    tracker.mark_discontinuity();
    // Новая цепочка начала счёт с нуля, а время ушло вперёд на ~50 периодов.
    tracker.record_present(0, MonotonicNanos::from_nanos(PERIOD_NANOS * 50));
    tracker.record_present(1, MonotonicNanos::from_nanos(PERIOD_NANOS * 51));
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.intervals, 2, "по одному интервалу в каждой эпохе");
    assert_eq!(snapshot.misses, 0, "время через разрыв пропуском не считается");
    assert_eq!(snapshot.timing_gaps, 0);
    assert_eq!(snapshot.histogram[1], 2);
}

/// Регрессия с телефона: Adreno отдаёт период с шумом (16666, 16675, 16674 мкс…), и любое
/// отличие очищало окно на каждом опросе, так что оно не копилось (вердикт навсегда
/// NotEnoughData). Шум меньше порога не должен менять период и трогать окно.
#[test]
fn jittering_reported_period_keeps_window_and_verdict_passes() {
    const PERIOD_60_HZ_NANOS: i64 = 16_666_000;
    let mut tracker = PacingTracker::new(PacingParams {
        refresh_period: Duration::from_nanos(PERIOD_60_HZ_NANOS as u64),
        max_miss_ratio: 0.005,
        window: Duration::from_secs(10),
        period_change_ratio: 0.02,
        min_coverage_ratio: 0.9,
    });
    let jitter_micros = [16_666u64, 16_675, 16_674, 16_673, 16_671];
    let mut now = 1_000_000_000i64;
    // 12 секунд ровных кадров; каждые 15 кадров «опрос» отдаёт новое дрожащее значение.
    for present_id in 0..720u32 {
        now += PERIOD_60_HZ_NANOS;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
        if present_id % 15 == 0 {
            let reported = jitter_micros[(present_id as usize / 15) % jitter_micros.len()];
            assert!(
                !tracker.set_refresh_period(Duration::from_micros(reported)),
                "дрожание {reported} мкс не смена периода"
            );
        }
    }
    assert_eq!(tracker.period_changes(), 0);
    assert_eq!(
        tracker.refresh_period(),
        Duration::from_nanos(PERIOD_60_HZ_NANOS as u64)
    );
    let snapshot = tracker.snapshot();
    assert!(snapshot.intervals > 500, "окно накопилось: {}", snapshot.intervals);
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
}

/// Настоящая смена режима (16,67 -> 8,33 мс) очищает окно ровно один раз, а повторное
/// сообщение того же нового периода (с шумом) — уже нет.
#[test]
fn real_period_switch_clears_window_exactly_once() {
    let mut tracker = PacingTracker::new(PacingParams {
        refresh_period: Duration::from_micros(16_667),
        max_miss_ratio: 0.005,
        window: Duration::from_secs(10),
        period_change_ratio: 0.02,
        min_coverage_ratio: 0.9,
    });
    let mut now = 1_000_000_000i64;
    for present_id in 0..30u32 {
        now += 16_667_000;
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
    }
    assert!(tracker.snapshot().intervals > 0);

    assert!(tracker.set_refresh_period(Duration::from_micros(8_333)));
    assert_eq!(tracker.snapshot(), PacingSnapshot::empty());
    assert!(
        !tracker.set_refresh_period(Duration::from_micros(8_340)),
        "шум нового режима"
    );
    assert!(!tracker.set_refresh_period(Duration::from_micros(8_330)));
    assert_eq!(tracker.period_changes(), 1);
    assert_eq!(tracker.refresh_period(), Duration::from_micros(8_333));
}

#[test]
fn set_refresh_period_rejects_zero_and_reports_no_change_for_same_value() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    let before = tracker.refresh_period();
    assert!(!tracker.set_refresh_period(Duration::ZERO));
    assert!(!tracker.set_refresh_period(before));
    assert_eq!(tracker.refresh_period(), before);
}

#[test]
fn sparse_window_with_many_lost_records_is_not_a_pass() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    // Окно набрано по времени (11 с), но показ виден лишь у каждого 5-го кадра:
    // 1300 меток, между соседними четыре потерянные записи -> покрытие 1/5.
    let mut now = 1_000_000_000i64;
    for index in 0..1300u32 {
        tracker.record_present(index * 5, MonotonicNanos::from_nanos(now));
        now += PERIOD_NANOS * 5;
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.intervals, 0, "соседних номеров нет вовсе");
    assert_eq!(snapshot.verdict, PacingVerdict::NotEnoughData);

    // Каждая 4-я пара соседняя: интервалов мало, окно набрано, пропусков нет.
    let mut tracker = PacingTracker::new(params(0.005, 10));
    let mut now = 1_000_000_000i64;
    let mut present_id = 0u32;
    for index in 0..1300u32 {
        tracker.record_present(present_id, MonotonicNanos::from_nanos(now));
        present_id += if index % 4 == 0 { 1 } else { 3 };
        now += PERIOD_NANOS;
    }
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.misses, 0);
    assert!(snapshot.intervals > 0 && snapshot.timing_gaps > snapshot.intervals);
    match snapshot.verdict {
        PacingVerdict::SparseData {
            coverage_ratio,
            min_coverage_ratio,
            intervals,
            frames_covered,
        } => {
            assert_eq!(min_coverage_ratio, 0.9);
            assert_eq!(
                (intervals, frames_covered),
                (snapshot.intervals, snapshot.frames_covered)
            );
            assert!((coverage_ratio - intervals as f64 / frames_covered as f64).abs() < 1e-12);
            assert!(coverage_ratio < 0.9);
        }
        other => panic!("ожидалось SparseData, получено {other:?}"),
    }
}

#[test]
fn phone_like_coverage_601_of_602_still_passes() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    feed(&mut tracker, 1300, |_| 0);
    let snapshot = tracker.snapshot();
    assert!(snapshot.intervals as f64 / snapshot.frames_covered as f64 > 0.99);
    assert_eq!(snapshot.verdict, PacingVerdict::Passed);
}

#[test]
fn snapshot_reports_rejected_out_of_order_marks() {
    let mut tracker = PacingTracker::new(params(0.005, 10));
    tracker.record_present(0, MonotonicNanos::from_nanos(2_000_000_000));
    tracker.record_present(1, MonotonicNanos::from_nanos(1_000_000_000));
    tracker.record_present(2, MonotonicNanos::from_nanos(1_500_000_000));
    let snapshot = tracker.snapshot();
    assert_eq!(snapshot.rejected_out_of_order, 2);
    assert_eq!(tracker.rejected_out_of_order(), snapshot.rejected_out_of_order);
}
