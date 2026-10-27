//! Адаптер подписки на vsync оболочки к трейту [`FrameClock`].
//!
//! Отображение один к одному: `VsyncWait::Tick` -> `FrameWait::Tick`, `TimedOut` -> `TimedOut`,
//! `Stopped` -> `Stopped`. Адаптер помнит номер последнего полученного тика и просит у ящика
//! только более свежие, поэтому один и тот же vsync никогда не приходит дважды, а пропущенные
//! тики честно попадают в `skipped_ticks`.

use std::time::Duration;

use shell_contract::vsync::{VsyncSlot, VsyncSubscription, VsyncWait};

use super::frame_clock::{FrameClock, FrameTick, FrameWait};

/// Часы кадров поверх настоящей подписки на vsync. Владеет подпиской: пока часы живы,
/// источник публикует тики, при уничтожении подписка останавливает производителя.
pub struct VsyncFrameClock {
    subscription: VsyncSubscription,
    /// Номер последнего тика, переданного потоку (`NOTHING_SEEN`, пока тиков не было).
    last_delivered_sequence: u64,
}

impl VsyncFrameClock {
    /// Создаёт часы; первый же опубликованный тик будет доставлен.
    pub fn new(subscription: VsyncSubscription) -> Self {
        Self {
            subscription,
            last_delivered_sequence: VsyncSlot::NOTHING_SEEN,
        }
    }
}

impl FrameClock for VsyncFrameClock {
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait {
        match self.subscription.wait_next(self.last_delivered_sequence, timeout) {
            VsyncWait::Tick { tick, skipped } => {
                self.last_delivered_sequence = tick.sequence;
                FrameWait::Tick(FrameTick {
                    sequence: tick.sequence,
                    expected_present: tick.expected_present,
                    deadline: tick.deadline,
                    skipped_ticks: skipped,
                    timelines: tick.timelines,
                })
            }
            VsyncWait::TimedOut => FrameWait::TimedOut,
            VsyncWait::Stopped => FrameWait::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    use shell_contract::clock::MonotonicNanos;
    use shell_contract::vsync::{FrameTimelines, VsyncTick, VsyncTimeline};

    use super::*;

    const WAIT: Duration = Duration::from_secs(2);

    fn tick(sequence: u64) -> VsyncTick {
        let base = i64::try_from(sequence).unwrap_or(0) * 8_333_333;
        // Две линии: ранняя (не предпочитаемая) и предпочитаемая — часы обязаны отдать обе.
        FrameTimelines::from_platform_list(2, 1, |index| {
            let offset = i64::try_from(index).unwrap_or(0) * 8_333_333;
            VsyncTimeline {
                vsync_id: i64::try_from(sequence).unwrap_or(0) * 10 + i64::try_from(index).unwrap_or(0),
                expected_present: MonotonicNanos::from_nanos(base + 1_666_667 + offset),
                deadline: MonotonicNanos::from_nanos(base - 4_333_333 + offset),
            }
        })
        .map(|timelines| VsyncTick::from_timelines(sequence, MonotonicNanos::from_nanos(base), timelines))
        .unwrap_or_else(|error| panic!("линии корректны: {error}"))
    }

    fn clock_over(slot: &Arc<VsyncSlot>) -> VsyncFrameClock {
        VsyncFrameClock::new(VsyncSubscription::new(Arc::clone(slot), Box::new(())))
    }

    /// Тики публикуются из ДРУГОГО потока ~120 Гц; часы отдают их по возрастанию и
    /// переносят expected_present/deadline без искажений.
    #[test]
    fn ticks_published_from_another_thread_arrive_in_order_with_their_timestamps() {
        let slot = Arc::new(VsyncSlot::new());
        let mut clock = clock_over(&slot);
        let producer_slot = Arc::clone(&slot);
        let producer = thread::spawn(move || {
            for sequence in VsyncSlot::FIRST_SEQUENCE..VsyncSlot::FIRST_SEQUENCE + 20 {
                producer_slot.publish(tick(sequence));
                thread::sleep(Duration::from_micros(8_333));
            }
        });

        let mut received = Vec::new();
        let started = Instant::now();
        while received.len() < 5 && started.elapsed() < WAIT {
            if let FrameWait::Tick(frame_tick) = clock.wait_next_frame(Duration::from_millis(50)) {
                received.push(frame_tick);
            }
        }
        assert!(producer.join().is_ok(), "поток-производитель не упал");
        assert!(received.len() >= 5, "получено тиков: {}", received.len());
        assert!(received.windows(2).all(|pair| pair[1].sequence > pair[0].sequence));
        for frame_tick in &received {
            let expected = tick(frame_tick.sequence);
            assert_eq!(frame_tick.expected_present, expected.expected_present);
            assert_eq!(frame_tick.deadline, expected.deadline);
            assert_eq!(frame_tick.timelines, expected.timelines, "все линии доходят до потока");
            assert_eq!(frame_tick.timelines.as_slice().len(), 2);
        }
    }

    #[test]
    fn missed_ticks_are_reported_as_skipped_and_same_tick_is_not_delivered_twice() {
        let slot = Arc::new(VsyncSlot::new());
        let mut clock = clock_over(&slot);
        slot.publish(tick(1));
        assert!(matches!(
            clock.wait_next_frame(WAIT),
            FrameWait::Tick(FrameTick {
                sequence: 1,
                skipped_ticks: 0,
                ..
            })
        ));
        // Потребитель «проспал» тики 2..=4: приходит только свежий, с подсчётом пропусков.
        slot.publish(tick(2));
        slot.publish(tick(3));
        slot.publish(tick(4));
        match clock.wait_next_frame(WAIT) {
            FrameWait::Tick(frame_tick) => {
                assert_eq!(frame_tick.sequence, 4);
                assert_eq!(frame_tick.skipped_ticks, 2);
            }
            other => panic!("ожидали тик, получили {other:?}"),
        }
        // Повторного тика нет: ждать нечего -> TimedOut.
        assert_eq!(clock.wait_next_frame(Duration::from_millis(20)), FrameWait::TimedOut);
    }

    #[test]
    fn stopped_slot_maps_to_stopped_and_wakes_a_waiting_thread() {
        let slot = Arc::new(VsyncSlot::new());
        let mut clock = clock_over(&slot);
        let stopper_slot = Arc::clone(&slot);
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            stopper_slot.stop();
        });
        let started = Instant::now();
        // Таймаут намного больше времени остановки: проснуться должны по Stopped, а не по таймауту.
        assert_eq!(clock.wait_next_frame(WAIT), FrameWait::Stopped);
        assert!(started.elapsed() < Duration::from_millis(1500));
        assert!(stopper.join().is_ok());
    }
}
