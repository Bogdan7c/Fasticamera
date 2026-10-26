//! Куда маршрутизатор передаёт события экрана: surface и жизненный цикл (решение владельца 31).
//!
//! Маршрутизатор сам surface больше не держит: он отдаёт её [`RenderRoute`]. Маршрутов два.
//! * [`InboxRoute`] — основной: почтовый ящик потока рендера (`render-loop`). Квитанцию освобождения
//!   подтверждает поток после `detach`.
//! * [`HoldAndConfirmRoute`] — запасной, если GPU или vsync не поднялись: держит surface (как это
//!   делал маршрутизатор в сессии 02) и подтверждает освобождение сразу, чтобы `surfaceDestroyed`
//!   в оболочке никогда не зависал.
//!
//! Все методы не блокируют: маршрутизатор — единственный читатель очереди событий оболочки.

use render_loop::mailbox::{InboxDelivery, RenderInbox};
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceReleaseRequest, SurfaceSize};

/// Получатель событий экрана.
pub trait RenderRoute: Send {
    /// Android создал surface; получатель становится её владельцем.
    fn surface_created(&mut self, surface: NativeSurface) -> InboxDelivery;

    /// Размер surface данного поколения.
    fn surface_changed(&mut self, generation: SurfaceGeneration, size: SurfaceSize) -> InboxDelivery;

    /// Android забирает surface: получатель обязан отпустить её и подтвердить квитанцию
    /// (или уничтожить квитанцию, если подтвердить нельзя).
    fn surface_destroyed(&mut self, request: SurfaceReleaseRequest) -> InboxDelivery;

    fn paused(&mut self) -> InboxDelivery;

    fn resumed(&mut self) -> InboxDelivery;
}

/// Основной маршрут: события уходят в почтовый ящик потока рендера.
pub struct InboxRoute {
    inbox: RenderInbox,
}

impl InboxRoute {
    pub fn new(inbox: RenderInbox) -> Self {
        Self { inbox }
    }
}

impl RenderRoute for InboxRoute {
    fn surface_created(&mut self, surface: NativeSurface) -> InboxDelivery {
        self.inbox.surface_created(surface)
    }

    fn surface_changed(&mut self, generation: SurfaceGeneration, size: SurfaceSize) -> InboxDelivery {
        self.inbox.surface_changed(generation, size)
    }

    fn surface_destroyed(&mut self, request: SurfaceReleaseRequest) -> InboxDelivery {
        self.inbox.surface_destroyed(request)
    }

    fn paused(&mut self) -> InboxDelivery {
        self.inbox.paused()
    }

    fn resumed(&mut self) -> InboxDelivery {
        self.inbox.resumed()
    }
}

/// Запасной маршрут (рендера нет): держит текущую surface и подтверждает освобождение сразу.
#[derive(Default)]
pub struct HoldAndConfirmRoute {
    held: Option<NativeSurface>,
}

impl HoldAndConfirmRoute {
    pub fn new() -> Self {
        Self::default()
    }

    /// Поколение удерживаемой surface (для тестов и диагностики).
    pub fn held_generation(&self) -> Option<SurfaceGeneration> {
        self.held.as_ref().map(NativeSurface::generation)
    }
}

impl RenderRoute for HoldAndConfirmRoute {
    fn surface_created(&mut self, surface: NativeSurface) -> InboxDelivery {
        if let Some(previous) = self.held.replace(surface) {
            tracing::warn!(
                target: "fasticamera::router",
                previous = previous.generation().value(),
                "новая surface без уничтожения прежней"
            );
        }
        InboxDelivery::Delivered
    }

    fn surface_changed(&mut self, _generation: SurfaceGeneration, _size: SurfaceSize) -> InboxDelivery {
        // Рисовать нечем: размер нужен только фактам экрана, их ведёт маршрутизатор.
        InboxDelivery::Delivered
    }

    fn surface_destroyed(&mut self, request: SurfaceReleaseRequest) -> InboxDelivery {
        // Устаревшее уничтожение (другое поколение) не должно отпускать более новую surface.
        if self.held_generation() == Some(request.generation()) {
            self.held = None;
        }
        request.confirm_released();
        InboxDelivery::Delivered
    }

    fn paused(&mut self) -> InboxDelivery {
        InboxDelivery::Delivered
    }

    fn resumed(&mut self) -> InboxDelivery {
        InboxDelivery::Delivered
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use shell_contract::surface::{PlatformWindow, SurfaceReleaseOutcome, surface_release_handoff};

    use super::*;

    /// Окно, которое говорит, что его уничтожили.
    struct TrackedWindow(Arc<AtomicBool>);

    impl PlatformWindow for TrackedWindow {
        fn raw_window(&self) -> NonNull<c_void> {
            NonNull::dangling()
        }
    }

    impl Drop for TrackedWindow {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn tracked_surface(generation: u64) -> (NativeSurface, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        let surface = NativeSurface::new(
            SurfaceGeneration::new(generation),
            Box::new(TrackedWindow(Arc::clone(&dropped))),
        );
        (surface, dropped)
    }

    #[test]
    fn fallback_holds_surface_until_destroyed_then_confirms_and_releases_window() {
        let mut route = HoldAndConfirmRoute::new();
        let (surface, dropped) = tracked_surface(1);
        route.surface_created(surface);
        assert_eq!(route.held_generation(), Some(SurfaceGeneration::new(1)));
        assert!(!dropped.load(Ordering::SeqCst), "окно живо, пока surface не уничтожена");

        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        assert_eq!(route.surface_destroyed(request), InboxDelivery::Delivered);
        assert_eq!(
            waiter.wait(Duration::from_millis(100)),
            SurfaceReleaseOutcome::Confirmed
        );
        assert!(dropped.load(Ordering::SeqCst), "окно отпущено до подтверждения");
        assert_eq!(route.held_generation(), None);
    }

    #[test]
    fn fallback_stale_destroy_confirms_but_keeps_newer_surface() {
        let mut route = HoldAndConfirmRoute::new();
        let (surface, dropped) = tracked_surface(2);
        route.surface_created(surface);

        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        route.surface_destroyed(request);
        assert_eq!(
            waiter.wait(Duration::from_millis(100)),
            SurfaceReleaseOutcome::Confirmed
        );
        assert_eq!(route.held_generation(), Some(SurfaceGeneration::new(2)));
        assert!(!dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn fallback_replaces_previous_surface_without_destroy() {
        let mut route = HoldAndConfirmRoute::new();
        let (first, first_dropped) = tracked_surface(1);
        let (second, _second_dropped) = tracked_surface(2);
        route.surface_created(first);
        route.surface_created(second);
        assert!(first_dropped.load(Ordering::SeqCst), "вытесненное окно отпущено");
        assert_eq!(route.held_generation(), Some(SurfaceGeneration::new(2)));
    }

    #[test]
    fn inbox_route_reports_receiver_gone_and_drops_receipt() {
        let (inbox, receiver) = render_loop::mailbox::render_mailbox();
        drop(receiver);
        let mut route = InboxRoute::new(inbox);
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        assert_eq!(route.surface_destroyed(request), InboxDelivery::ReceiverGone);
        assert_eq!(
            waiter.wait(Duration::from_millis(100)),
            SurfaceReleaseOutcome::DroppedWithoutConfirmation,
            "оболочка не ждёт таймаут, если поток рендера мёртв"
        );
    }
}
