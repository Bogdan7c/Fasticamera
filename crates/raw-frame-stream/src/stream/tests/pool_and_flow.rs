//! План пула и сквозной путь кадра до потребителя.

use super::support::{Rig, latest_wins_config, signal, unsignaled_fence};
use crate::config::RawFrameStreamConfig;
use crate::stream::{OfferOutcome, ViewfinderPoll};

const RAW10_4096X3072_BYTES: u64 = 15_728_640;

/// Буфер видоискателя (решение 53) добавил к плану сессии 05 (6 буферов) одну ячейку: очередь 2
/// вместо ячейки 1. План один и тот же при любой стартовой политике — переключение на ходу.
/// Кольцо — только в конфиге по умолчанию (решение 139): 12 кадров + 2 удержания снимков.
#[test]
fn default_pool_plan_sums_to_twenty_one_buffers_with_the_ring_of_decision_139() {
    let default_plan = Rig::new(RawFrameStreamConfig::default()).start.pool;
    let pool = default_plan;
    assert_eq!(
        (
            pool.viewfinder,
            pool.pending_pairs,
            pool.snapshot_ring,
            pool.snapshot_capture_holds,
            pool.lossless,
            pool.slack
        ),
        (4, 2, 12, 2, 0, 1)
    );
    assert_eq!(pool.reader_max_images(), 21);
    // Та же политика видоискателя на старте не меняет план: переключение на ходу.
    let latest_wins = Rig::new(RawFrameStreamConfig {
        viewfinder_pacing: crate::viewfinder_pacing::ViewfinderPacing::LatestWins,
        ..RawFrameStreamConfig::default()
    })
    .start
    .pool;
    assert_eq!(latest_wins, default_plan);
}

#[test]
fn capture_holds_count_toward_max_images_only_when_the_ring_is_enabled() {
    let without_ring = Rig::new(latest_wins_config()).start.pool;
    assert_eq!(
        (without_ring.snapshot_ring, without_ring.snapshot_capture_holds),
        (0, 0)
    );
    assert_eq!(
        without_ring.reader_max_images(),
        4 + 2 + 1,
        "кольца нет — удержаний снимков нет"
    );

    let with_ring = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 3 * RAW10_4096X3072_BYTES,
        snapshot_capture_holds: 2,
        ..latest_wins_config()
    })
    .start
    .pool;
    assert_eq!((with_ring.snapshot_ring, with_ring.snapshot_capture_holds), (3, 2));
    assert_eq!(with_ring.reader_max_images(), 4 + 2 + 3 + 2 + 1);

    // План, собранный снаружи с кольцом 0, тоже не считает удержания.
    let hand_made = crate::PoolPlan {
        snapshot_capture_holds: 5,
        ..without_ring
    };
    assert_eq!(hand_made.reader_max_images(), without_ring.reader_max_images());
}

#[test]
fn ring_and_lossless_queue_grow_the_pool_to_fifteen_buffers() {
    let mut rig = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 4 * RAW10_4096X3072_BYTES,
        ..latest_wins_config()
    });
    let _lossless = rig.stream.attach_lossless_consumer(2.try_into().expect("ёмкость"));
    assert_eq!(
        rig.start.pool.lossless, 0,
        "очередь входит в план со следующего begin_stream"
    );
    rig.restart();
    assert_eq!(
        rig.start.pool.reader_max_images(),
        15,
        "4+2 + кольцо 4 + удержания 2 + очередь 2 + запас 1"
    );
}

#[test]
fn a_frame_travels_from_the_camera_to_the_renderer_with_its_own_metadata_and_goes_back_after_the_gpu() {
    let rig = Rig::new(latest_wins_config());
    let viewfinder = rig.stream.viewfinder();

    // Камера: кадр, потом результат. Рендер: опрос, импорт буфера, возврат с fence-ом GPU.
    assert_eq!(rig.offer_frame(16_000, 0), OfferOutcome::AwaitingResult);
    rig.offer_result(16_000, 2);
    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_for_vsync(rig.at(3)) else {
        panic!("кадр должен дойти до видоискателя");
    };

    let details = lease.passport().capture.details().expect("метаданные этого кадра");
    assert_eq!(details.result.frame_number, super::support::frame_number_for(16_000));
    assert_eq!(
        lease.passport().stream.raw_format,
        camera_contract::raw_format::RawFormat::Raw10
    );
    let _imported = lease.hardware_buffer().as_ptr();
    assert!(lease.acquire_fence().is_ok());
    assert!(
        !rig.log.is_returned(16_000),
        "пока GPU читает, камера буфер не получает"
    );

    let (gpu_done, mut gpu_signal) = unsignaled_fence();
    lease.release_after_gpu_fence(gpu_done);
    assert!(!rig.log.is_free(16_000));
    signal(&mut gpu_signal);
    assert!(rig.log.is_free(16_000));

    let counters = rig.stream.counters();
    assert_eq!(
        (
            counters.viewfinder_delivered,
            counters.cpu_copies,
            counters.leaked_leases
        ),
        (1, 0, 0)
    );
    assert_eq!(counters.frames_outstanding, 0);
}

#[test]
fn viewfinder_handles_share_one_slot() {
    let rig = Rig::new(latest_wins_config());
    let first = rig.stream.viewfinder();
    let second = rig.stream.viewfinder();
    rig.offer_result(1_000, 0);
    rig.offer_frame(1_000, 0);

    let ViewfinderPoll::NewFrame(lease) = first.poll_for_vsync(rig.at(0)) else {
        panic!("кадр должен быть в общей ячейке");
    };
    assert!(matches!(second.poll_for_vsync(rig.at(0)), ViewfinderPoll::NoNewFrame));
    lease.release_unused();
}

#[test]
fn cpu_copies_are_counted() {
    let rig = Rig::new(latest_wins_config());
    rig.producer.note_cpu_copy();
    rig.producer.note_cpu_copy();
    assert_eq!(rig.stream.counters().cpu_copies, 2);
}
