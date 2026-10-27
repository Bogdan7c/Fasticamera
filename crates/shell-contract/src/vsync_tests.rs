//! Тесты ящика и подписки vsync: свежесть тика, учёт пропусков, ожидание с таймаутом,
//! остановка, межпоточное пробуждение и остановка производителя при `Drop` подписки.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::*;

/// Достаточно долго, чтобы тест не был хрупким, но провал не вешал прогон.
const GENEROUS_DEADLINE: Duration = Duration::from_secs(5);
/// Короткое ожидание для проверки «тика нет».
const SHORT_WAIT: Duration = Duration::from_millis(30);

fn tick(sequence: u64) -> VsyncTick {
    let base = i64::try_from(sequence).unwrap_or(i64::MAX) * 8_333_333;
    VsyncTick::from_timelines(
        sequence,
        MonotonicNanos::from_nanos(base),
        FrameTimelines::only(VsyncTimeline {
            vsync_id: i64::try_from(sequence).unwrap_or(0) + 1000,
            expected_present: MonotonicNanos::from_nanos(base + 16_666_666),
            deadline: MonotonicNanos::from_nanos(base + 8_000_000),
        }),
    )
}

/// Линия с номером `index`: показ и срок растут на период 120 Гц с каждой линией.
fn timeline_at(index: usize) -> VsyncTimeline {
    let step = i64::try_from(index).unwrap_or(i64::MAX) * 8_333_333;
    VsyncTimeline {
        vsync_id: 500 + i64::try_from(index).unwrap_or(0),
        expected_present: MonotonicNanos::from_nanos(20_000_000 + step),
        deadline: MonotonicNanos::from_nanos(12_000_000 + step),
    }
}

#[test]
fn tick_flat_fields_are_the_preferred_timeline_and_all_timelines_are_kept() {
    let timelines = FrameTimelines::from_platform_list(3, 1, timeline_at).expect("список корректен");
    let tick = VsyncTick::from_timelines(7, MonotonicNanos::from_nanos(5), timelines);
    assert_eq!(tick.expected_present, timeline_at(1).expected_present);
    assert_eq!(tick.deadline, timeline_at(1).deadline);
    assert_eq!(tick.vsync_id, timeline_at(1).vsync_id);
    assert_eq!(
        tick.timelines.as_slice(),
        &[timeline_at(0), timeline_at(1), timeline_at(2)]
    );
    assert_eq!(tick.timelines.preferred_index(), 1);
    assert_eq!(tick.timelines.dropped_count(), 0);
}

#[test]
fn more_timelines_than_capacity_are_truncated_with_a_count_keeping_the_earliest() {
    let mut read_indices = Vec::new();
    let timelines = FrameTimelines::from_platform_list(MAX_FRAME_TIMELINES + 3, 2, |index| {
        read_indices.push(index);
        timeline_at(index)
    })
    .expect("список корректен");
    assert_eq!(timelines.dropped_count(), 3, "отброшенные линии посчитаны");
    assert_eq!(timelines.as_slice().len(), MAX_FRAME_TIMELINES);
    assert_eq!(timelines.as_slice()[0], timeline_at(0), "ранние линии сохранены");
    assert_eq!(timelines.preferred(), timeline_at(2));
    assert_eq!(
        read_indices,
        (0..MAX_FRAME_TIMELINES).collect::<Vec<_>>(),
        "лишние не читались"
    );
}

#[test]
fn preferred_beyond_capacity_stays_in_the_window_and_its_index_is_rebased() {
    let preferred_platform_index = MAX_FRAME_TIMELINES + 1;
    let timelines = FrameTimelines::from_platform_list(MAX_FRAME_TIMELINES + 4, preferred_platform_index, timeline_at)
        .expect("список корректен");
    assert_eq!(timelines.preferred(), timeline_at(preferred_platform_index));
    assert_eq!(
        timelines.preferred_index(),
        MAX_FRAME_TIMELINES - 1,
        "окно кончается на предпочитаемой"
    );
    assert_eq!(timelines.as_slice()[0], timeline_at(2), "две самые ранние отброшены");
    assert_eq!(timelines.dropped_count(), 4);
}

#[test]
fn empty_list_and_preferred_out_of_range_are_distinct_errors() {
    assert_eq!(
        FrameTimelines::from_platform_list(0, 0, timeline_at),
        Err(FrameTimelinesError::NoTimelines)
    );
    assert_eq!(
        FrameTimelines::from_platform_list(2, 2, timeline_at),
        Err(FrameTimelinesError::PreferredIndexOutOfRange {
            preferred_index: 2,
            timeline_count: 2
        })
    );
}

/// Производитель-заглушка: флаг выставляется, когда подписку уничтожили.
struct StopFlagOnDrop(Arc<AtomicBool>);

impl Drop for StopFlagOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[test]
fn wait_returns_published_tick_with_all_fields() {
    let slot = VsyncSlot::new();
    slot.publish(tick(1));
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(1),
            skipped: 0
        },
        slot.wait_next(VsyncSlot::NOTHING_SEEN, GENEROUS_DEADLINE)
    );
}

#[test]
fn wait_returns_freshest_tick_and_reports_skipped_count() {
    let slot = VsyncSlot::new();
    for sequence in 1..=5 {
        slot.publish(tick(sequence));
    }
    // Потребитель видел только №1: тики 2, 3, 4 для него потеряны, отдаётся №5.
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(5),
            skipped: 3
        },
        slot.wait_next(1, GENEROUS_DEADLINE)
    );
}

#[test]
fn first_wait_counts_ticks_before_it_as_skipped() {
    let slot = VsyncSlot::new();
    slot.publish(tick(4));
    // Из №1..3 потребитель не видел ничего.
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(4),
            skipped: 3
        },
        slot.wait_next(VsyncSlot::NOTHING_SEEN, GENEROUS_DEADLINE)
    );
}

#[test]
fn already_seen_tick_is_not_delivered_twice() {
    let slot = VsyncSlot::new();
    slot.publish(tick(1));
    assert!(matches!(slot.wait_next(0, GENEROUS_DEADLINE), VsyncWait::Tick { .. }));
    // Тот же тик повторно не отдаётся: ждём новый и получаем таймаут.
    assert_eq!(VsyncWait::TimedOut, slot.wait_next(1, SHORT_WAIT));
}

#[test]
fn wait_times_out_without_ticks_after_at_least_the_timeout() {
    let slot = VsyncSlot::new();
    let started = Instant::now();
    assert_eq!(VsyncWait::TimedOut, slot.wait_next(0, SHORT_WAIT));
    assert!(started.elapsed() >= SHORT_WAIT, "ожидание закончилось раньше таймаута");
}

#[test]
fn zero_timeout_polls_without_blocking() {
    let slot = VsyncSlot::new();
    assert_eq!(VsyncWait::TimedOut, slot.wait_next(0, Duration::ZERO));
    slot.publish(tick(1));
    assert!(matches!(slot.wait_next(0, Duration::ZERO), VsyncWait::Tick { .. }));
}

#[test]
fn huge_timeout_does_not_overflow_and_still_returns_on_stop() {
    let slot = Arc::new(VsyncSlot::new());
    let waiter_slot = Arc::clone(&slot);
    let waiter = thread::spawn(move || waiter_slot.wait_next(0, Duration::MAX));
    thread::sleep(SHORT_WAIT);
    slot.stop();
    assert_eq!(Ok(VsyncWait::Stopped), waiter.join().map_err(|_| ()));
}

#[test]
fn stop_wakes_blocked_waiter_with_stopped() {
    let slot = Arc::new(VsyncSlot::new());
    let waiter_slot = Arc::clone(&slot);
    let waiter = thread::spawn(move || waiter_slot.wait_next(0, GENEROUS_DEADLINE));
    thread::sleep(SHORT_WAIT);
    let stopped_at = Instant::now();
    slot.stop();
    assert_eq!(Ok(VsyncWait::Stopped), waiter.join().map_err(|_| ()));
    assert!(
        stopped_at.elapsed() < GENEROUS_DEADLINE,
        "остановка не разбудила ожидающего"
    );
    assert!(slot.is_stopped());
}

#[test]
fn stop_takes_priority_over_a_pending_tick_and_blocks_further_publishing() {
    let slot = VsyncSlot::new();
    slot.publish(tick(1));
    slot.stop();
    assert_eq!(VsyncWait::Stopped, slot.wait_next(0, GENEROUS_DEADLINE));
    slot.publish(tick(2));
    assert_eq!(1, slot.counters().published, "после стопа публикация не принимается");
}

#[test]
fn publish_from_another_thread_wakes_blocked_waiter_within_deadline() {
    let slot = Arc::new(VsyncSlot::new());
    let producer_slot = Arc::clone(&slot);
    let producer = thread::spawn(move || {
        thread::sleep(SHORT_WAIT);
        producer_slot.publish(tick(7));
    });
    let started = Instant::now();
    let outcome = slot.wait_next(VsyncSlot::NOTHING_SEEN, GENEROUS_DEADLINE);
    assert!(producer.join().is_ok());
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(7),
            skipped: 6
        },
        outcome
    );
    assert!(started.elapsed() < GENEROUS_DEADLINE);
}

#[test]
fn older_or_equal_sequence_is_ignored_and_counted() {
    let slot = VsyncSlot::new();
    slot.publish(tick(5));
    slot.publish(tick(5));
    slot.publish(tick(3));
    slot.publish(tick(0));
    let counters = slot.counters();
    assert_eq!(1, counters.published);
    assert_eq!(3, counters.stale_ignored);
    // Свежий тик не «откатился» к более старому.
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(5),
            skipped: 4
        },
        slot.wait_next(0, GENEROUS_DEADLINE)
    );
}

#[test]
fn overwritten_counts_only_ticks_nobody_took() {
    let slot = VsyncSlot::new();
    slot.publish(tick(1));
    slot.publish(tick(2)); // №1 никто не забрал: перезапись.
    assert!(matches!(slot.wait_next(0, GENEROUS_DEADLINE), VsyncWait::Tick { .. }));
    slot.publish(tick(3)); // №2 забрали: перезаписью не считается.
    assert_eq!(
        VsyncSlotCounters {
            published: 3,
            overwritten: 1,
            stale_ignored: 0,
            delivered: 1
        },
        slot.counters()
    );
}

#[test]
fn dropping_subscription_stops_producer_and_wakes_slot_waiters() {
    let slot = Arc::new(VsyncSlot::new());
    let producer_stopped = Arc::new(AtomicBool::new(false));
    let subscription = VsyncSubscription::new(
        Arc::clone(&slot),
        Box::new(StopFlagOnDrop(Arc::clone(&producer_stopped))),
    );
    slot.publish(tick(1));
    assert_eq!(
        VsyncWait::Tick {
            tick: tick(1),
            skipped: 0
        },
        subscription.wait_next(0, GENEROUS_DEADLINE)
    );
    assert!(
        !producer_stopped.load(Ordering::Acquire),
        "пока подписка жива, производитель работает"
    );
    drop(subscription);
    assert!(
        producer_stopped.load(Ordering::Acquire),
        "уничтожение подписки должно остановить производителя"
    );
    assert_eq!(VsyncWait::Stopped, slot.wait_next(1, GENEROUS_DEADLINE));
}

/// Fake-источник по трейту: доказывает, что контракт `VsyncSource` собирается из частей модуля,
/// и что подписка отдаёт то, что производитель публикует из своего потока.
struct FakeVsyncSource;

impl VsyncSource for FakeVsyncSource {
    fn start_vsync(&self) -> Result<VsyncSubscription, VsyncSourceError> {
        let slot = Arc::new(VsyncSlot::new());
        let producer_slot = Arc::clone(&slot);
        let producer = thread::spawn(move || {
            for sequence in VsyncSlot::FIRST_SEQUENCE..=3 {
                producer_slot.publish(tick(sequence));
                thread::sleep(Duration::from_millis(5));
            }
        });
        struct JoinOnDrop(Option<thread::JoinHandle<()>>);
        impl Drop for JoinOnDrop {
            fn drop(&mut self) {
                if let Some(handle) = self.0.take()
                    && handle.join().is_err()
                {
                    eprintln!("производитель fake-источника запаниковал");
                }
            }
        }
        Ok(VsyncSubscription::new(slot, Box::new(JoinOnDrop(Some(producer)))))
    }
}

#[test]
fn fake_source_delivers_increasing_ticks_through_the_subscription() {
    let source: &dyn VsyncSource = &FakeVsyncSource;
    let subscription = source.start_vsync().expect("fake всегда запускается");
    let mut last_seen = VsyncSlot::NOTHING_SEEN;
    while last_seen < 3 {
        match subscription.wait_next(last_seen, GENEROUS_DEADLINE) {
            VsyncWait::Tick { tick, .. } => {
                assert!(tick.sequence > last_seen, "порядок тиков нарушен");
                last_seen = tick.sequence;
            }
            other => panic!("ожидался тик, получено {other:?}"),
        }
    }
    assert_eq!(3, last_seen);
}

#[test]
fn source_errors_are_distinguishable_and_explain_themselves() {
    let unsupported = VsyncSourceError::UnsupportedApiLevel {
        required: 33,
        actual: 30,
    };
    assert_ne!(unsupported, VsyncSourceError::NoLooper);
    assert!(unsupported.to_string().contains("33"));
    assert!(
        VsyncSourceError::PlatformFailure {
            reason: "сбой".to_owned()
        }
        .to_string()
        .contains("сбой")
    );
}
