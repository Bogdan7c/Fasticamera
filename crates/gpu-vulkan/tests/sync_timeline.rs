//! Тесты timeline-семафора: CPU-ожидание, монотонность, ожидание на стороне GPU.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use std::time::Duration;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::sync::{QueueSubmission, TimelineSemaphore, TimelineWait};

const STAGE: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::ALL_COMMANDS;
const LONG_TIMEOUT: Duration = Duration::from_secs(5);
const SHORT_TIMEOUT: Duration = Duration::from_millis(50);

#[test]
fn cpu_wait_reaches_value_after_submit_and_completed_value_is_monotonic() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let timeline = TimelineSemaphore::new(&context, 0).unwrap();
    assert_eq!(timeline.completed_value().unwrap(), 0);

    let mut previous_completed = 0;
    for value in [1_u64, 2, 5, 100] {
        QueueSubmission::new()
            .signal_timeline(&timeline, value, STAGE)
            .submit(&context)
            .unwrap();
        assert_eq!(timeline.wait_cpu(value, LONG_TIMEOUT).unwrap(), TimelineWait::Reached);
        let completed = timeline.completed_value().unwrap();
        assert!(completed >= value, "completed {completed} < ожидаемого {value}");
        assert!(
            completed >= previous_completed,
            "счётчик убыл: {previous_completed} -> {completed}"
        );
        previous_completed = completed;
        assert_eq!(timeline.last_submitted_value(), value);
    }
    // Значение, которое никто не сигналит, не достигается.
    assert_eq!(timeline.wait_cpu(101, SHORT_TIMEOUT).unwrap(), TimelineWait::TimedOut);
    assert_eq!(timeline.completed_value().unwrap(), 100);

    drop(timeline);
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

#[test]
fn wait_times_out_when_nothing_signals() {
    let Some(context) = gpu_context_or_skip() else { return };
    let timeline = TimelineSemaphore::new(&context, 0).unwrap();
    assert_eq!(timeline.wait_cpu(1, SHORT_TIMEOUT).unwrap(), TimelineWait::TimedOut);
    assert_eq!(timeline.completed_value().unwrap(), 0);
}

#[test]
fn non_increasing_signal_value_is_rejected_before_the_driver_and_changes_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let timeline = TimelineSemaphore::new(&context, 3).unwrap();
    for rejected in [0_u64, 2, 3] {
        let error = QueueSubmission::new()
            .signal_timeline(&timeline, rejected, STAGE)
            .submit(&context)
            .unwrap_err();
        assert!(
            matches!(error, GpuError::InvalidArgument { .. }),
            "{rejected}: {error:?}"
        );
        assert_eq!(timeline.last_submitted_value(), 3);
    }
    assert_eq!(timeline.completed_value().unwrap(), 3);
    assert_no_validation_errors(&context);
}

/// GPU-сторона: сабмит ждёт значение одного семафора (ворота) и только после него сигналит другой.
#[test]
fn submission_waits_for_timeline_value_on_the_gpu() {
    let Some(context) = gpu_context_or_skip() else { return };
    let gate = TimelineSemaphore::new(&context, 0).unwrap();
    let result = TimelineSemaphore::new(&context, 0).unwrap();

    QueueSubmission::new()
        .wait_for_timeline(&gate, 5, STAGE)
        .signal_timeline(&result, 1, STAGE)
        .submit(&context)
        .unwrap();
    // Ворота закрыты: результат не должен появиться.
    assert_eq!(result.wait_cpu(1, SHORT_TIMEOUT).unwrap(), TimelineWait::TimedOut);

    // Ворота открывает CPU: на одной очереди сабмиты идут по порядку, и второй сабмит
    // не смог бы обогнать первый, который стоит в ожидании.
    gate.signal_from_cpu(5).unwrap();
    assert_eq!(result.wait_cpu(1, LONG_TIMEOUT).unwrap(), TimelineWait::Reached);
    assert_no_validation_errors(&context);
}

#[test]
fn concurrent_signals_of_the_same_value_let_exactly_one_submit_through() {
    let Some(context) = gpu_context_or_skip() else { return };
    const THREADS: usize = 8;
    let timeline = TimelineSemaphore::new(&context, 0).unwrap();
    let start = std::sync::Barrier::new(THREADS);

    let accepted = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    QueueSubmission::new()
                        .signal_timeline(&timeline, 7, STAGE)
                        .submit(&context)
                        .is_ok()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("поток не паникует"))
            .filter(|accepted| *accepted)
            .count()
    });

    // Проверка «больше отправленного» и запись значения — один атомарный шаг: значение 7 может
    // уйти на GPU только один раз (VUID-VkSubmitInfo2-semaphore-03882), остальные получают ошибку.
    assert_eq!(accepted, 1, "значение сигнала принято {accepted} раз");
    assert_eq!(timeline.wait_cpu(7, LONG_TIMEOUT).unwrap(), TimelineWait::Reached);
    assert_eq!(timeline.last_submitted_value(), 7);
    drop(timeline);
    assert_no_validation_errors(&context);
}
