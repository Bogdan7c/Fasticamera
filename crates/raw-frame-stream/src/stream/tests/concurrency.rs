//! Конкурентный прогон: камера и рендер в разных потоках.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use shell_contract::sync_fence::GpuReleaseFence;

use super::support::{ReturnLog, frame_for, result_for};
use crate::config::RawFrameStreamConfig;
use crate::stream::{RawFrameStream, ViewfinderPoll};

const FRAMES: i64 = 2_000;
const SHUFFLE_WINDOW: usize = 4;

/// События камеры (кадр = true, результат = false), перемешанные внутри небольших окон.
fn shuffled_camera_events() -> Vec<(bool, i64)> {
    let mut events: Vec<(bool, i64)> = (1..=FRAMES)
        .flat_map(|index| [(true, index * 1_000), (false, index * 1_000)])
        .collect();
    let mut seed = 12_345_u64;
    for window in events.chunks_mut(SHUFFLE_WINDOW) {
        for position in (1..window.len()).rev() {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let other = usize::try_from((seed >> 33) % (position as u64 + 1)).expect("индекс");
            window.swap(position, other);
        }
    }
    events
}

#[test]
fn camera_and_renderer_threads_return_every_buffer_exactly_once() {
    let stream = RawFrameStream::new(RawFrameStreamConfig::default());
    let producer = stream.producer();
    let viewfinder = stream.viewfinder();
    let log = Arc::new(ReturnLog::default());
    let start = producer.begin_stream(Arc::new(super::support::passport(
        camera_contract::raw_format::RawFormat::Raw10,
        camera_contract::geometry::PixelSize::new(4096, 3072),
    )));
    let camera_done = Arc::new(AtomicBool::new(false));

    let renderer = {
        let camera_done = Arc::clone(&camera_done);
        thread::spawn(move || {
            let (mut received, mut wrong_metadata, mut turn) = (0_u32, 0_u32, 0_u32);
            loop {
                let finished = camera_done.load(Ordering::Acquire);
                match viewfinder.poll_latest(Instant::now()) {
                    ViewfinderPoll::NewFrame(lease) => {
                        received += 1;
                        let passport = lease.passport();
                        let metadata_matches = passport
                            .capture
                            .details()
                            .is_none_or(|details| details.result.sensor_timestamp == Some(passport.sensor_timestamp));
                        if !metadata_matches {
                            wrong_metadata += 1;
                        }
                        turn += 1;
                        if turn % 2 == 0 {
                            lease.release_unused();
                        } else {
                            lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
                        }
                    }
                    ViewfinderPoll::NoNewFrame | ViewfinderPoll::StreamInactive if finished => break,
                    ViewfinderPoll::NoNewFrame | ViewfinderPoll::StreamInactive => thread::yield_now(),
                }
            }
            (received, wrong_metadata)
        })
    };

    let camera = {
        let producer = producer.clone();
        let log = Arc::clone(&log);
        thread::spawn(move || {
            for (is_frame, timestamp) in shuffled_camera_events() {
                if is_frame {
                    producer.offer_frame(start.epoch, frame_for(&log, timestamp), Instant::now());
                } else {
                    producer.offer_capture_result(start.epoch, result_for(timestamp), Instant::now());
                }
            }
        })
    };

    camera.join().expect("поток камеры");
    // Всё, что ещё ждёт пару, публикуется по таймауту (час спустя по часам теста).
    producer.expire_pending(Instant::now() + Duration::from_secs(3600));
    camera_done.store(true, Ordering::Release);
    let (received, wrong_metadata) = renderer.join().expect("поток рендера");
    producer.end_stream(start.epoch);

    let counters = stream.counters();
    assert!(received > 0, "рендер должен был получить кадры");
    assert_eq!(
        wrong_metadata, 0,
        "метаданные чужого кадра не должны попадать к потребителю"
    );
    assert_eq!(counters.frames_offered, FRAMES as u64);
    assert_eq!(
        counters.frames_returned, counters.frames_offered,
        "каждый буфер вернулся ровно раз"
    );
    assert_eq!(log.returned_count() as u64, counters.frames_offered);
    assert_eq!(counters.frames_outstanding, 0);
    assert_eq!(counters.leaked_leases, 0);
    assert!(counters.max_frames_outstanding <= u64::from(start.pool.reader_max_images()));
}
