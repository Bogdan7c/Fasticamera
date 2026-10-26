//! Тесты кольца кадров на реальном GPU (профиль Headless, полная validation).

use std::time::{Duration, Instant};

use ash::vk;

use super::*;
use crate::context::test_support::gpu_context_or_skip;
use crate::live_objects::LiveObjectKind;

fn frames(count: usize) -> FramesInFlight {
    FramesInFlight::new(count).expect("допустимое число кадров")
}

#[test]
fn timeout_nanos_saturates_below_vulkan_infinite_wait() {
    assert_eq!(timeout_nanos(Duration::from_millis(100)), 100_000_000);
    assert_eq!(timeout_nanos(Duration::ZERO), 0);
    // Никакая длительность не превращается в u64::MAX — «бесконечное ожидание» Vulkan.
    assert_eq!(timeout_nanos(Duration::MAX), i64::MAX as u64);
    assert!(timeout_nanos(Duration::from_secs(u64::MAX / 2)) < u64::MAX);
}

#[test]
fn ring_creates_exactly_the_declared_objects_and_releases_them() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let ring = FrameRing::new(&context, frames(3), 5).unwrap();
        let live = context.live_objects();
        assert_eq!(ring.frames_in_flight(), 3);
        assert_eq!(ring.render_finished_count(), 5);
        // 3 семафора acquire + 5 render-finished, 3 fence, 1 пул.
        assert_eq!(
            live.count(LiveObjectKind::Semaphore),
            baseline.count(LiveObjectKind::Semaphore) + 8
        );
        assert_eq!(
            live.count(LiveObjectKind::Fence),
            baseline.count(LiveObjectKind::Fence) + 3
        );
        assert_eq!(
            live.count(LiveObjectKind::CommandPool),
            baseline.count(LiveObjectKind::CommandPool) + 1
        );
        assert_ne!(ring.pool_handle(), vk::CommandPool::null());
    }
    assert_eq!(context.live_objects(), baseline, "всё освобождено");
}

#[test]
fn recreating_render_finished_changes_only_the_per_image_semaphores() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut ring = FrameRing::new(&context, frames(2), 3).unwrap();
    let before = context.live_objects();
    ring.recreate_render_finished(5).unwrap();
    let after = context.live_objects();
    assert_eq!(ring.render_finished_count(), 5);
    assert_eq!(
        after.count(LiveObjectKind::Semaphore),
        before.count(LiveObjectKind::Semaphore) + 2
    );
    assert_eq!(after.count(LiveObjectKind::Fence), before.count(LiveObjectKind::Fence));
    assert!(matches!(
        ring.render_finished_semaphore(5),
        Err(GpuError::InvalidArgument { .. })
    ));
}

#[test]
fn free_slots_are_handed_out_and_busy_slot_times_out_within_budget() {
    let Some(context) = gpu_context_or_skip() else { return };
    let ring = FrameRing::new(&context, frames(2), 2).unwrap();
    // Свежий слот свободен (fence создан сигнальным).
    assert!(matches!(
        ring.wait_for_next_slot(Duration::from_millis(50)),
        Ok(SlotWait::Ready(_))
    ));
    // Имитируем «GPU ещё не закончил»: сбрасываем fence слота без сабмита — он никогда не сигналится.
    // SAFETY: fence принадлежит устройству и не используется GPU.
    unsafe {
        context
            .device()
            .reset_fences(&[ring.slots[0].in_flight.handle()])
            .unwrap()
    };
    let started = Instant::now();
    let outcome = ring.wait_for_next_slot(Duration::from_millis(60)).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(outcome, SlotWait::TimedOut);
    assert!(
        elapsed >= Duration::from_millis(55),
        "вернулись раньше таймаута: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "ожидание не ограничено таймаутом: {elapsed:?}"
    );
    // Общее ожидание тоже ограничено и сообщает Timeout, а не зависает.
    assert!(matches!(
        ring.wait_for_idle(Duration::from_millis(30)),
        Err(GpuError::Timeout { .. })
    ));
}

#[test]
fn empty_submit_signals_slot_fence_and_advances_ring() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut ring = FrameRing::new(&context, frames(2), 2).unwrap();
    let SlotWait::Ready(slot) = ring.wait_for_next_slot(Duration::from_millis(50)).unwrap() else {
        panic!("слот должен быть свободен");
    };
    // Эмулируем acquire: сигналим семафор слота пустым сабмитом, который ждёт end_and_submit.
    let acquired = ring.image_acquired_semaphore(slot);
    let signal = [vk::SemaphoreSubmitInfo::default()
        .semaphore(acquired)
        .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
    context
        .submit(
            &[vk::SubmitInfo2::default().signal_semaphore_infos(&signal)],
            vk::Fence::null(),
        )
        .unwrap();
    ring.begin_recording(slot).unwrap();
    ring.end_and_submit(slot, 0).unwrap();

    // После сабмита fence слота 0 должен сигналиться сам, а кольцо — перейти к слоту 1.
    ring.wait_for_idle(Duration::from_secs(5)).unwrap();
    assert_eq!(ring.next_slot, 1);
    assert!(matches!(
        ring.wait_for_next_slot(Duration::from_millis(50)),
        Ok(SlotWait::Ready(next)) if next != slot
    ));
    context.wait_idle().unwrap();
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}
