//! Тесты импорта/экспорта sync_fd: отказ возвращает fence, повторные сигналы отвергаются, CPU-ожидание.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;

use std::io::{ErrorKind, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use ash::vk;
use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::sync::{
    ExportableSemaphore, FenceWait, GpuReleaseFence, GpuWaitFence, QueueSubmission, WaitSemaphore, import_wait_fence,
    wait_release_fence_on_cpu,
};

const STAGE: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::ALL_COMMANDS;
const SHORT_TIMEOUT: Duration = Duration::from_secs(5);

/// fd, который не является sync-файлом: конец сокета + наблюдатель за его закрытием.
fn non_sync_fd() -> (OwnedFd, UnixStream) {
    let (ours, observer) = UnixStream::pair().unwrap();
    observer.set_nonblocking(true).unwrap();
    (OwnedFd::from(ours), observer)
}

fn assert_peer_open(observer: &mut UnixStream) {
    let mut byte = [0_u8; 1];
    assert_eq!(observer.read(&mut byte).unwrap_err().kind(), ErrorKind::WouldBlock);
}

#[test]
fn import_failure_returns_the_fence_untouched_and_leaks_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let (fd, mut observer) = non_sync_fd();

    let error = import_wait_fence(&context, GpuWaitFence::from_sync_fd(fd)).unwrap_err();

    assert!(matches!(error.reason, GpuError::Vulkan { .. }), "{:?}", error.reason);
    // Fence вернулся вместе с дескриптором, который драйвер не забрал и не закрыл.
    assert!(!error.fence.is_already_signaled());
    assert_peer_open(&mut observer);
    // Созданный для импорта семафор уничтожен.
    assert_eq!(context.live_objects(), baseline);
    // Владение у нас: после drop дескриптор закрывается ровно один раз.
    drop(error);
    let mut byte = [0_u8; 1];
    assert_eq!(observer.read(&mut byte).unwrap(), 0, "ожидали EOF после drop fence");
    assert_no_validation_errors(&context);
}

#[test]
fn already_signaled_fence_imports_to_no_wait_without_creating_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let wait = import_wait_fence(&context, GpuWaitFence::already_signaled()).unwrap();
    assert!(matches!(wait, WaitSemaphore::NoWait));
    assert!(wait.semaphore().is_none());
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn real_exported_fence_imports_and_is_waited_by_a_submission() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let producer = ExportableSemaphore::new(&context).unwrap();
    let mut work = QueueSubmission::new()
        .signal_for_export(&producer, STAGE)
        .submit(&context)
        .unwrap();
    let release = work.take_release_signal().unwrap().export().unwrap();

    // Сабмит без команд мог завершиться мгновенно (-1) или дать настоящий fd — принимаем оба,
    // но в любом случае импорт и ожидание в следующем сабмите должны пройти.
    let wait_fence = into_wait(release);
    let imported = import_wait_fence(&context, wait_fence).unwrap();
    let consumer = ExportableSemaphore::new(&context).unwrap();
    let mut second = QueueSubmission::new()
        .wait_for_imported_fence(&imported, STAGE)
        .signal_for_export(&consumer, STAGE)
        .submit(&context)
        .unwrap();
    let second_release = second.take_release_signal().unwrap().export().unwrap();
    assert_eq!(
        wait_release_fence_on_cpu(&context, &second_release, SHORT_TIMEOUT).unwrap(),
        FenceWait::Signaled
    );

    context.wait_idle().unwrap();
    drop((imported, producer, consumer, second_release));
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

fn into_wait(release: GpuReleaseFence) -> GpuWaitFence {
    match release.into_owned_fd() {
        Some(fd) => GpuWaitFence::from_sync_fd(fd),
        None => GpuWaitFence::already_signaled(),
    }
}

#[test]
fn exportable_semaphore_cannot_be_signaled_twice_before_export() {
    let Some(context) = gpu_context_or_skip() else { return };
    let semaphore = ExportableSemaphore::new(&context).unwrap();
    assert!(!semaphore.is_awaiting_export());

    let mut first = QueueSubmission::new()
        .signal_for_export(&semaphore, STAGE)
        .submit(&context)
        .unwrap();
    assert!(semaphore.is_awaiting_export());

    // Второй сигнал до экспорта отвергается типом, не доходя до драйвера.
    let error = QueueSubmission::new()
        .signal_for_export(&semaphore, STAGE)
        .submit(&context)
        .unwrap_err();
    assert!(matches!(error, GpuError::InvalidState { .. }), "{error:?}");
    assert!(
        semaphore.is_awaiting_export(),
        "отказ не должен сбрасывать пометку первого сигнала"
    );

    // После экспорта семафор снова доступен.
    drop(first.take_release_signal().unwrap().export().unwrap());
    assert!(!semaphore.is_awaiting_export());
    let mut again = QueueSubmission::new()
        .signal_for_export(&semaphore, STAGE)
        .submit(&context)
        .unwrap();
    drop(again.take_release_signal().unwrap().export().unwrap());

    context.wait_idle().unwrap();
    assert_no_validation_errors(&context);
}

#[test]
fn two_export_semaphores_in_one_submission_are_rejected_without_side_effects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let first = ExportableSemaphore::new(&context).unwrap();
    let second = ExportableSemaphore::new(&context).unwrap();
    let error = QueueSubmission::new()
        .signal_for_export(&first, STAGE)
        .signal_for_export(&second, STAGE)
        .submit(&context)
        .unwrap_err();
    assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
    assert!(!first.is_awaiting_export());
    assert!(!second.is_awaiting_export());
}

#[test]
fn already_signaled_release_fence_does_not_wait_and_non_sync_fd_is_rejected_by_driver() {
    let Some(context) = gpu_context_or_skip() else { return };
    assert_eq!(
        wait_release_fence_on_cpu(&context, &GpuReleaseFence::already_signaled(), Duration::ZERO).unwrap(),
        FenceWait::Signaled
    );
    let (fd, mut observer) = non_sync_fd();
    let fake = GpuReleaseFence::from_sync_fd(fd);
    let baseline = context.live_objects();
    let error = wait_release_fence_on_cpu(&context, &fake, Duration::ZERO).unwrap_err();
    assert!(matches!(error, GpuError::Vulkan { .. }), "{error:?}");
    // Наш дескриптор остался открытым (дублировали и закрыли дубликат), fence цел.
    assert_peer_open(&mut observer);
    assert!(fake.as_sync_fd().is_some());
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}
