//! Возврат буферов производителю: доли, fence-ы, утечка.

use super::support::{Rig, latest_wins_config, signal, unsignaled_fence};
use crate::config::RawFrameStreamConfig;
use crate::lease::RawFrameLease;
use crate::stream::ViewfinderPoll;
use shell_contract::sync_fence::GpuReleaseFence;

const RAW10_4096X3072_BYTES: u64 = 15_728_640;

/// Один кадр, который держат и видоискатель, и кольцо.
fn frame_held_by_viewfinder_and_ring(timestamp: i64) -> (Rig, RawFrameLease, RawFrameLease) {
    let rig = Rig::new(RawFrameStreamConfig {
        snapshot_ring_budget_bytes: 2 * RAW10_4096X3072_BYTES,
        ..latest_wins_config()
    });
    rig.offer_result(timestamp, 0);
    rig.offer_frame(timestamp, 0);
    let ViewfinderPoll::NewFrame(from_viewfinder) = rig.stream.viewfinder().poll_for_vsync(rig.at(0)) else {
        panic!("кадр должен быть в ячейке");
    };
    let mut from_ring = rig.stream.snapshot_ring().take_all();
    assert_eq!(from_ring.len(), 1);
    let from_ring = from_ring.remove(0);
    (rig, from_viewfinder, from_ring)
}

#[test]
fn shared_buffer_returns_to_the_producer_only_after_the_last_holder_releases() {
    let (rig, viewfinder_lease, ring_lease) = frame_held_by_viewfinder_and_ring(1_000);

    viewfinder_lease.release_unused();
    assert!(!rig.log.is_returned(1_000), "кольцо ещё держит буфер");
    ring_lease.release_unused();

    assert_eq!(rig.log.fence_count(1_000), Some(0));
    assert_eq!(rig.stream.counters().frames_returned, 1);
}

#[test]
fn fences_of_all_holders_reach_the_producer_and_the_pool_waits_for_both_signals() {
    let (rig, viewfinder_lease, ring_lease) = frame_held_by_viewfinder_and_ring(2_000);
    let (viewfinder_fence, mut viewfinder_signal) = unsignaled_fence();
    let (ring_fence, mut ring_signal) = unsignaled_fence();

    viewfinder_lease.release_after_gpu_fence(viewfinder_fence);
    ring_lease.release_after_gpu_fence(ring_fence);

    assert_eq!(rig.log.fence_count(2_000), Some(2), "оба fence-а переданы камере");
    assert!(!rig.log.is_free(2_000), "GPU ещё не закончил");
    signal(&mut viewfinder_signal);
    assert!(!rig.log.is_free(2_000), "второй fence ещё не сигнален");
    signal(&mut ring_signal);
    assert!(rig.log.is_free(2_000), "оба сигнала пришли -> буфер свободен");
    assert_eq!(rig.stream.counters().released_after_fence, 2);
}

#[test]
fn an_already_signaled_fence_is_not_forwarded() {
    let (rig, viewfinder_lease, ring_lease) = frame_held_by_viewfinder_and_ring(3_000);
    viewfinder_lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
    ring_lease.release_unused();
    assert_eq!(rig.log.fence_count(3_000), Some(0));
    assert!(rig.log.is_free(3_000));
}

#[test]
fn dropping_a_lease_without_release_leaks_the_buffer_even_if_others_release_properly() {
    let (rig, viewfinder_lease, ring_lease) = frame_held_by_viewfinder_and_ring(4_000);

    drop(viewfinder_lease); // забыли вернуть
    ring_lease.release_unused();

    assert!(
        !rig.log.is_returned(4_000),
        "GPU мог читать буфер: камере его не отдаём"
    );
    let counters = rig.stream.counters();
    assert_eq!((counters.leaked_leases, counters.frames_leaked), (1, 1));
    assert_eq!(counters.frames_returned, 0);
    assert_eq!(counters.frames_outstanding, 0);
}

#[test]
fn lease_exposes_the_buffer_and_a_usable_acquire_fence() {
    let (rig, viewfinder_lease, ring_lease) = frame_held_by_viewfinder_and_ring(5_000);
    let fence = viewfinder_lease.acquire_fence().expect("копия acquire-fence");
    assert!(fence.is_already_signaled());
    // Оба лизинга показывают один и тот же буфер.
    assert_eq!(
        viewfinder_lease.hardware_buffer().as_ptr(),
        ring_lease.hardware_buffer().as_ptr()
    );
    viewfinder_lease.release_unused();
    ring_lease.release_unused();
    assert!(rig.log.is_returned(5_000));
}
