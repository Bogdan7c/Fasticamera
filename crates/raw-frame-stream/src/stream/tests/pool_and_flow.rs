//! План пула и сквозной путь кадра до потребителя.

use super::support::{Rig, signal, unsignaled_fence};
use crate::config::RawFrameStreamConfig;
use crate::stream::{OfferOutcome, ViewfinderPoll};

const RAW10_4096X3072_BYTES: u64 = 15_728_640;

#[test]
fn default_pool_plan_sums_to_six_buffers() {
    let rig = Rig::new(RawFrameStreamConfig::default());
    let pool = rig.start.pool;
    assert_eq!(
        (
            pool.viewfinder,
            pool.pending_pairs,
            pool.snapshot_ring,
            pool.lossless,
            pool.slack
        ),
        (3, 2, 0, 0, 1)
    );
    assert_eq!(pool.reader_max_images(), 6);
}

#[test]
fn ring_and_lossless_queue_grow_the_pool_to_twelve_buffers() {
    let mut rig = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 4 * RAW10_4096X3072_BYTES,
        ..RawFrameStreamConfig::default()
    });
    let _lossless = rig.stream.attach_lossless_consumer(2.try_into().expect("ёмкость"));
    assert_eq!(
        rig.start.pool.lossless, 0,
        "очередь входит в план со следующего begin_stream"
    );
    rig.restart();
    assert_eq!(rig.start.pool.reader_max_images(), 12);
}

#[test]
fn a_frame_travels_from_the_camera_to_the_renderer_with_its_own_metadata_and_goes_back_after_the_gpu() {
    let rig = Rig::new(RawFrameStreamConfig::default());
    let viewfinder = rig.stream.viewfinder();

    // Камера: кадр, потом результат. Рендер: опрос, импорт буфера, возврат с fence-ом GPU.
    assert_eq!(rig.offer_frame(16_000, 0), OfferOutcome::AwaitingResult);
    rig.offer_result(16_000, 2);
    let ViewfinderPoll::NewFrame(lease) = viewfinder.poll_latest(rig.at(3)) else {
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
    let rig = Rig::new(RawFrameStreamConfig::default());
    let first = rig.stream.viewfinder();
    let second = rig.stream.viewfinder();
    rig.offer_result(1_000, 0);
    rig.offer_frame(1_000, 0);

    let ViewfinderPoll::NewFrame(lease) = first.poll_latest(rig.at(0)) else {
        panic!("кадр должен быть в общей ячейке");
    };
    assert!(matches!(second.poll_latest(rig.at(0)), ViewfinderPoll::NoNewFrame));
    lease.release_unused();
}

#[test]
fn cpu_copies_are_counted() {
    let rig = Rig::new(RawFrameStreamConfig::default());
    rig.producer.note_cpu_copy();
    rig.producer.note_cpu_copy();
    assert_eq!(rig.stream.counters().cpu_copies, 2);
}
