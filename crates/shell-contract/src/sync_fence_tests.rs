//! Тесты владения `sync_fd`. Настоящие дескрипторы берём из `UnixStream::pair()`: закрытие
//! одного конца наблюдаемо со второго (чтение даёт EOF), а живой конец даёт `WouldBlock`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

use std::io::{ErrorKind, Read};
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

use super::*;

/// Пара «наш конец как fd» + «наблюдатель», который видит, закрыт ли наш конец.
fn fd_with_observer() -> (OwnedFd, UnixStream) {
    let (ours, observer) = UnixStream::pair().unwrap();
    observer.set_nonblocking(true).unwrap();
    (OwnedFd::from(ours), observer)
}

/// Конец ещё открыт: чтение без данных блокировалось бы.
fn assert_peer_open(observer: &mut UnixStream) {
    let mut byte = [0_u8; 1];
    let error = observer.read(&mut byte).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::WouldBlock, "ожидали открытый fd");
}

/// Конец закрыт: чтение возвращает EOF.
fn assert_peer_closed(observer: &mut UnixStream) {
    let mut byte = [0_u8; 1];
    assert_eq!(observer.read(&mut byte).unwrap(), 0, "ожидали закрытый fd (EOF)");
}

#[test]
fn wait_fence_closes_descriptor_exactly_when_dropped() {
    let (fd, mut observer) = fd_with_observer();
    let fence = GpuWaitFence::from_sync_fd(fd);
    assert!(!fence.is_already_signaled());
    assert_peer_open(&mut observer);
    drop(fence);
    assert_peer_closed(&mut observer);
}

#[test]
fn wait_fence_into_owned_fd_keeps_descriptor_open_for_new_owner() {
    let (fd, mut observer) = fd_with_observer();
    let fence = GpuWaitFence::from_sync_fd(fd);
    let handed_over = fence.into_owned_fd().expect("у несигнального fence есть fd");
    assert_peer_open(&mut observer);
    drop(handed_over);
    assert_peer_closed(&mut observer);
}

#[test]
fn already_signaled_wait_fence_has_no_descriptor() {
    let fence = GpuWaitFence::already_signaled();
    assert!(fence.is_already_signaled());
    assert!(fence.as_sync_fd().is_none());
    assert!(fence.into_owned_fd().is_none());
}

#[test]
fn ndk_minus_one_means_already_signaled() {
    // SAFETY: -1 — не дескриптор, владение не передаётся.
    let fence = unsafe { GpuWaitFence::from_raw_ndk_fd(-1) }.unwrap();
    assert!(fence.is_already_signaled());
}

#[test]
fn other_negative_ndk_values_are_rejected() {
    for raw in [-2, -9, i32::MIN] {
        // SAFETY: отрицательное значение не принимается во владение.
        let error = unsafe { GpuWaitFence::from_raw_ndk_fd(raw) }.unwrap_err();
        assert_eq!(error, SyncFenceError::InvalidNdkFd { raw });
    }
}

#[test]
fn raw_ndk_fd_transfers_ownership_into_fence() {
    let (fd, mut observer) = fd_with_observer();
    // SAFETY: fd только что получен из into_raw_fd и больше нигде не закрывается.
    let fence = unsafe { GpuWaitFence::from_raw_ndk_fd(fd.into_raw_fd()) }.unwrap();
    assert!(!fence.is_already_signaled());
    assert_peer_open(&mut observer);
    drop(fence);
    assert_peer_closed(&mut observer);
}

#[test]
fn release_fence_closes_descriptor_when_dropped() {
    let (fd, mut observer) = fd_with_observer();
    let fence = GpuReleaseFence::from_sync_fd(fd);
    assert!(fence.as_sync_fd().is_some());
    assert_peer_open(&mut observer);
    drop(fence);
    assert_peer_closed(&mut observer);
}

#[test]
fn release_fence_into_raw_ndk_fd_moves_ownership_to_the_caller() {
    let (fd, mut observer) = fd_with_observer();
    let fence = GpuReleaseFence::from_sync_fd(fd);
    let raw = fence.into_raw_ndk_fd();
    assert!(raw >= 0);
    // Fence потреблён, но дескриптор жив: он у «NDK» (в тесте — у нас).
    assert_peer_open(&mut observer);
    // SAFETY: raw только что отдан into_raw_ndk_fd, других владельцев нет.
    drop(unsafe { OwnedFd::from_raw_fd(raw) });
    assert_peer_closed(&mut observer);
}

#[test]
fn signaled_release_fence_gives_minus_one_to_ndk() {
    let fence = GpuReleaseFence::already_signaled();
    assert!(fence.is_already_signaled());
    assert_eq!(fence.into_raw_ndk_fd(), -1);
}
