//! Синтетическая платформа ПК-стенда `latency_profiles_bench.rs`: vsync 60,00 Гц с линиями кадра как у
//! Choreographer и камера 60,03 к/с с дрожанием ПРИХОДА кадра (факты OnePlus 11, сессия 07).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "стенд тестов: падение — сигнал о провале"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use camera_contract::capture_result::SensorTimestamp;
use raw_frame_stream::{OfferedFrame, RawFrameStream};
use render_loop::thread::{MonotonicTimer, SleepUntil, SystemMonotonicTimer, VsyncFrameClock, sleep_until};
use shell_contract::clock::MonotonicNanos;
use shell_contract::sync_fence::GpuWaitFence;
use shell_contract::vsync::{FrameTimelines, VsyncSlot, VsyncSubscription, VsyncTick, VsyncTimeline};

use super::camera_rig::{EventLog, FakeFrame, ProducerBook, result_for, stream_passport};

// ---- параметры синтетики (факты телефона, см. модуль) ----

/// Период vsync 60,00 Гц, нс.
pub const VSYNC_PERIOD_NANOS: i64 = 16_666_667;
/// Линий кадра в тике и индекс предпочитаемой.
pub const TIMELINE_COUNT: usize = 4;
pub const PREFERRED_TIMELINE: usize = 1;
/// Период кадров камеры 60,03 к/с, нс.
pub const CAMERA_PERIOD_NANOS: f64 = 1e9 / 60.03;
/// Возраст кадра при приходе: середина и полуразмах равномерного дрожания, нс.
pub const ARRIVAL_AGE_MID_NANOS: i64 = 35_000_000;
pub const ARRIVAL_AGE_HALF_SPREAD_NANOS: i64 = 8_000_000;
/// Зерно генератора дрожания (одинаковое для всех окон: профили сравниваются на одной последовательности).
pub const JITTER_SEED: u64 = 0x5EED_CAFE_F00D_0007;
/// Срок линии раньше её vsync (пробуждение SurfaceFlinger), нс.
pub const DEADLINE_BEFORE_VSYNC_NANOS: i64 = 2_000_000;

// ---- генератор дрожания ----

/// xorshift64*: детерминированный, без внешних crate-ов.
pub struct Jitter(pub u64);

impl Jitter {
    fn next_unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Возраст кадра при приходе, нс: равномерно в [mid − spread; mid + spread].
    pub fn arrival_age_nanos(&mut self) -> i64 {
        let offset = (self.next_unit() * 2.0 - 1.0) * ARRIVAL_AGE_HALF_SPREAD_NANOS as f64;
        ARRIVAL_AGE_MID_NANOS + offset.round() as i64
    }
}

pub fn monotonic_now() -> i64 {
    SystemMonotonicTimer.now().expect("монотонные часы").as_nanos()
}

/// Спит до момента `target_nanos` (`CLOCK_MONOTONIC`) с коротким активным дожиданием.
pub fn sleep_to(target_nanos: i64) {
    let _ = sleep_until(
        SleepUntil {
            target: MonotonicNanos::from_nanos(target_nanos),
            spin_window: Duration::from_micros(200),
            max_wait: Duration::from_secs(1),
        },
        &SystemMonotonicTimer,
    );
}

// ---- синтетический vsync с линиями кадра ----

pub struct VsyncPublisher {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for VsyncPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            assert!(join.join().is_ok(), "поток vsync упал");
        }
    }
}

pub fn timelines_at(vsync_nanos: i64) -> FrameTimelines {
    FrameTimelines::from_platform_list(TIMELINE_COUNT, PREFERRED_TIMELINE, |index| {
        let periods = i64::try_from(index).expect("индекс") + 1;
        let vsync = vsync_nanos + periods * VSYNC_PERIOD_NANOS;
        VsyncTimeline {
            vsync_id: vsync / VSYNC_PERIOD_NANOS,
            deadline: MonotonicNanos::from_nanos(vsync - DEADLINE_BEFORE_VSYNC_NANOS),
            expected_present: MonotonicNanos::from_nanos(vsync + VSYNC_PERIOD_NANOS),
        }
    })
    .expect("линии кадра корректны")
}

/// Тики vsync в моменты `anchor + k·P` (`anchor` общий с камерой: так задаётся фаза камеры к vsync).
pub fn vsync_clock_60_hz(anchor_nanos: i64) -> VsyncFrameClock {
    let slot = Arc::new(VsyncSlot::new());
    let stop = Arc::new(AtomicBool::new(false));
    let (producer_slot, producer_stop) = (Arc::clone(&slot), Arc::clone(&stop));
    let join = thread::Builder::new()
        .name("bench-vsync".to_owned())
        .spawn(move || {
            let first = anchor_nanos;
            let mut sequence = VsyncSlot::FIRST_SEQUENCE;
            while !producer_stop.load(Ordering::SeqCst) {
                let index = i64::try_from(sequence - VsyncSlot::FIRST_SEQUENCE).expect("номер");
                let vsync = first + index * VSYNC_PERIOD_NANOS;
                sleep_to(vsync);
                producer_slot.publish(VsyncTick::from_timelines(
                    sequence,
                    MonotonicNanos::from_nanos(vsync),
                    timelines_at(vsync),
                ));
                sequence += 1;
            }
        })
        .expect("поток vsync");
    let publisher = VsyncPublisher { stop, join: Some(join) };
    VsyncFrameClock::new(VsyncSubscription::new(slot, Box::new(publisher)))
}

// ---- камера 60,03 к/с с дрожанием прихода ----

pub struct JitteredCamera {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<u64>>,
}

impl JitteredCamera {
    /// Камера: метка кадра `k` = `anchor + phase + k·Pк`, приход = метка + возраст.
    pub fn start(
        stream: &RawFrameStream,
        log: &Arc<EventLog>,
        book: &Arc<ProducerBook>,
        anchor_nanos: i64,
        phase_nanos: i64,
    ) -> Self {
        let producer = stream.producer();
        let started = producer.begin_stream(Arc::new(stream_passport()));
        let epoch = started.epoch;
        let max_images = usize::try_from(started.pool.reader_max_images()).expect("пул");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let (log, book) = (Arc::clone(log), Arc::clone(book));
        let join = thread::Builder::new()
            .name("bench-camera".to_owned())
            .spawn(move || {
                let mut jitter = Jitter(JITTER_SEED);
                // Метки — на шкале CLOCK_MONOTONIC (на ПК без сна BOOTTIME = MONOTONIC), чтобы возраст
                // считался вычитанием. Фаза к vsync задаётся сдвигом от общего якоря.
                let first_sensor = anchor_nanos + phase_nanos;
                let mut sequence = 0_i64;
                let mut offered = 0_usize;
                while !thread_stop.load(Ordering::SeqCst) {
                    let sensor_ts = first_sensor + (sequence as f64 * CAMERA_PERIOD_NANOS).round() as i64;
                    sleep_to(sensor_ts + jitter.arrival_age_nanos());
                    // Камера не отдаёт больше буферов, чем в пуле (кадр «потерян сенсором» — не наш случай).
                    if offered.saturating_sub(book.returned_count()) < max_images {
                        let now = Instant::now();
                        let frame = OfferedFrame {
                            sensor_timestamp: SensorTimestamp::from_nanos(sensor_ts),
                            acquire_fence: GpuWaitFence::already_signaled(),
                            frame: Box::new(FakeFrame {
                                sensor_ts,
                                storage: Box::new(0),
                                log: Arc::clone(&log),
                                book: Arc::clone(&book),
                            }),
                        };
                        producer.offer_frame(epoch, frame, now);
                        producer.offer_capture_result(epoch, result_for(sensor_ts, sequence), now);
                        offered += 1;
                    }
                    sequence += 1;
                }
                u64::try_from(offered).unwrap_or(0)
            })
            .expect("поток камеры");
        Self { stop, join: Some(join) }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            join.join().expect("поток камеры не упал");
        }
    }
}
