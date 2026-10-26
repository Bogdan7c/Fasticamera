//! Surface экрана и контракт её освобождения.
//!
//! Android отдаёт surface `SurfaceView` в `surfaceCreated` и забирает в `surfaceDestroyed`.
//! `surfaceDestroyed` синхронный: после возврата из него surface трогать нельзя, поэтому
//! владелец рендера обязан отпустить swapchain до того, как оболочка вернёт управление
//! Android. Оболочка передаёт ему [`SurfaceReleaseRequest`] и ждёт подтверждения через
//! [`SurfaceReleaseWaiter`] с таймаутом (подробности — `android-shell`, сессия 02;
//! подтверждение от рендера — сессия 03).
//!
//! Тип окна платформы скрыт за [`PlatformWindow`]: нейтральный код хранит и передаёт surface,
//! а сырой указатель `ANativeWindow*` нужен только `gpu-vulkan` для `vkCreateAndroidSurfaceKHR`.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use serde::Serialize;

/// Номер surface по порядку появления (с 1). Отличает новую surface от старой, если события
/// о них пришли вперемешку: квитанция и размеры всегда относятся к конкретному номеру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SurfaceGeneration(u64);

impl SurfaceGeneration {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Размер surface в физических пикселях (как его сообщил `surfaceChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SurfaceSize {
    pub width_px: u32,
    pub height_px: u32,
}

/// Окно платформы, на котором рисует рендер. Реализация держит собственную ссылку
/// на окно (на Android — `ANativeWindow_acquire`) и отпускает её в `Drop`.
pub trait PlatformWindow: Send {
    /// Сырой указатель окна для Vulkan (`ANativeWindow*`). Действителен, пока жив `self`.
    fn raw_window(&self) -> NonNull<c_void>;
}

/// Surface экрана с номером поколения. Пока значение живо, окно не будет уничтожено
/// под ногами (счётчик ссылок), но после [`SurfaceReleaseRequest`] рисовать на нём нельзя.
pub struct NativeSurface {
    generation: SurfaceGeneration,
    window: Box<dyn PlatformWindow>,
}

impl NativeSurface {
    pub fn new(generation: SurfaceGeneration, window: Box<dyn PlatformWindow>) -> Self {
        Self { generation, window }
    }

    pub fn generation(&self) -> SurfaceGeneration {
        self.generation
    }

    /// Сырой указатель окна для создания swapchain. Действителен, пока жив `self`.
    pub fn raw_window(&self) -> NonNull<c_void> {
        self.window.raw_window()
    }
}

impl fmt::Debug for NativeSurface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeSurface")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Чем закончилось ожидание освобождения surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceReleaseOutcome {
    /// Владелец рендера явно подтвердил, что отпустил surface.
    Confirmed,
    /// Квитанция уничтожена без подтверждения: событие не дошло (очередь полна, получатель
    /// завершился) или владелец забыл подтвердить. Ждать дальше нечего.
    DroppedWithoutConfirmation,
    /// Подтверждения нет за отведённое время: рендер завис или не успел.
    TimedOut,
}

/// Внутренний сигнал квитанции.
#[derive(Debug, Clone, Copy)]
enum ReleaseSignal {
    Confirmed,
    Dropped,
}

/// Квитанция «surface поколения N уходит». Владелец рендера отпускает всё, что рисует
/// на этой surface, и вызывает [`SurfaceReleaseRequest::confirm_released`].
/// Уничтожение без подтверждения не оставляет оболочку ждать до таймаута: ожидающая
/// сторона сразу получает [`SurfaceReleaseOutcome::DroppedWithoutConfirmation`].
#[derive(Debug)]
pub struct SurfaceReleaseRequest {
    generation: SurfaceGeneration,
    signal: Option<SyncSender<ReleaseSignal>>,
}

impl SurfaceReleaseRequest {
    pub fn generation(&self) -> SurfaceGeneration {
        self.generation
    }

    /// Подтверждает, что surface больше не используется.
    pub fn confirm_released(mut self) {
        self.send(ReleaseSignal::Confirmed);
    }

    fn send(&mut self, signal: ReleaseSignal) {
        if let Some(sender) = self.signal.take() {
            // Ожидающая сторона могла уже уйти по таймауту — тогда сигнал никому не нужен,
            // а факт таймаута она сама учла. Канал на одно место, поэтому send не блокирует.
            let _receiver_gone = sender.try_send(signal);
        }
    }
}

impl Drop for SurfaceReleaseRequest {
    fn drop(&mut self) {
        self.send(ReleaseSignal::Dropped);
    }
}

/// Ожидающая сторона квитанции (у оболочки, в `surfaceDestroyed`).
#[derive(Debug)]
pub struct SurfaceReleaseWaiter {
    receiver: Receiver<ReleaseSignal>,
}

impl SurfaceReleaseWaiter {
    /// Ждёт подтверждения не дольше `timeout`.
    pub fn wait(self, timeout: Duration) -> SurfaceReleaseOutcome {
        match self.receiver.recv_timeout(timeout) {
            Ok(ReleaseSignal::Confirmed) => SurfaceReleaseOutcome::Confirmed,
            // Disconnected возможен, только если квитанцию забыли (mem::forget): Drop не сработал.
            Ok(ReleaseSignal::Dropped) | Err(RecvTimeoutError::Disconnected) => {
                SurfaceReleaseOutcome::DroppedWithoutConfirmation
            }
            Err(RecvTimeoutError::Timeout) => SurfaceReleaseOutcome::TimedOut,
        }
    }
}

/// Создаёт пару «квитанция для владельца рендера — ожидание для оболочки».
pub fn surface_release_handoff(generation: SurfaceGeneration) -> (SurfaceReleaseRequest, SurfaceReleaseWaiter) {
    // Одно место: сигнал отправляется ровно один раз, и отправитель никогда не ждёт.
    let (sender, receiver) = sync_channel(1);
    (
        SurfaceReleaseRequest {
            generation,
            signal: Some(sender),
        },
        SurfaceReleaseWaiter { receiver },
    )
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    const GENERATION: SurfaceGeneration = SurfaceGeneration::new(3);

    #[test]
    fn confirmation_from_other_thread_is_seen_by_waiter() {
        let (request, waiter) = surface_release_handoff(GENERATION);
        let render_thread = thread::spawn(move || {
            assert_eq!(request.generation(), GENERATION);
            request.confirm_released();
        });
        assert_eq!(waiter.wait(Duration::from_secs(5)), SurfaceReleaseOutcome::Confirmed);
        render_thread.join().expect("поток рендера");
    }

    #[test]
    fn dropped_request_wakes_waiter_immediately() {
        let (request, waiter) = surface_release_handoff(GENERATION);
        drop(request);
        // Таймаут большой: если бы Drop не сигналил, тест ждал бы час, а не проходил мгновенно.
        assert_eq!(
            waiter.wait(Duration::from_secs(3600)),
            SurfaceReleaseOutcome::DroppedWithoutConfirmation
        );
    }

    #[test]
    fn missing_confirmation_times_out() {
        let (request, waiter) = surface_release_handoff(GENERATION);
        assert_eq!(waiter.wait(Duration::from_millis(20)), SurfaceReleaseOutcome::TimedOut);
        // Поздняя квитанция после таймаута не паникует и не блокирует.
        request.confirm_released();
    }

    struct FakeWindow;

    impl PlatformWindow for FakeWindow {
        fn raw_window(&self) -> NonNull<c_void> {
            NonNull::dangling()
        }
    }

    #[test]
    fn native_surface_keeps_generation_and_window() {
        let surface = NativeSurface::new(GENERATION, Box::new(FakeWindow));
        assert_eq!(surface.generation(), GENERATION);
        assert_eq!(surface.raw_window(), NonNull::dangling());
        assert!(format!("{surface:?}").contains("generation"));
    }
}
