//! Почтовый ящик потока рендера: мост от маршрутизатора событий к потоку `fasticamera-render`.
//!
//! Что гарантирует ящик (инварианты):
//! * Сторона маршрутизатора ([`RenderInbox`]) никогда не ждёт ничего, кроме короткого замка
//!   на push в `Vec`: поток рендера не держит замок во время работы с GPU.
//! * События жизненного цикла (создание/смена/уничтожение surface, пауза) НЕ теряются:
//!   это не ограниченная очередь, а упорядоченный список. Он не растёт без предела, потому что
//!   подряд идущие `changed` одного поколения схлопываются, а подряд идущие пауза/возобновление
//!   сводятся к последнему из них (итоговое состояние, а не пара «взаимно уничтожилась»).
//! * Квитанция освобождения никогда не «глотается»: если поток рендера уже завершён, запрос
//!   уничтожается сразу, и ожидающая оболочка получает `DroppedWithoutConfirmation` без
//!   таймаута (см. [`SurfaceReleaseRequest`]).
//! * Простаивающий поток (нет surface) спит на `Condvar` и просыпается от любого события.

use std::mem;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceReleaseRequest, SurfaceSize};

/// Событие жизненного цикла для потока рендера.
#[derive(Debug)]
pub enum InboxEvent {
    /// Android создал surface. Размера у неё ещё нет — он придёт в [`InboxEvent::SurfaceChanged`].
    SurfaceCreated(NativeSurface),
    /// Размер surface (первый — запускает показ, следующие — пересоздание swapchain).
    SurfaceChanged {
        generation: SurfaceGeneration,
        size: SurfaceSize,
    },
    /// Android забирает surface: поток обязан отпустить её и подтвердить квитанцию.
    SurfaceDestroyed(SurfaceReleaseRequest),
    /// Показ приостановлен (surface остаётся присоединённой).
    Paused,
    /// Показ возобновлён.
    Resumed,
}

/// Событие с моментом постановки в ящик: по нему считается задержка освобождения surface,
/// как её видит оболочка (включая время, пока поток спал или рисовал кадр).
#[derive(Debug)]
pub struct StampedEvent {
    pub event: InboxEvent,
    pub enqueued_at: Instant,
}

/// Что поток забрал из ящика за один раз.
#[derive(Debug, Default)]
pub struct InboxBatch {
    /// События в порядке поступления.
    pub events: Vec<StampedEvent>,
    /// Запрошена остановка потока (события выше нужно обработать до выхода).
    pub stop_requested: bool,
}

/// Результат доставки события в ящик.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxDelivery {
    /// Событие лежит в ящике и дойдёт до потока.
    Delivered,
    /// Поток рендера уже завершён: событие отброшено (для `surface_destroyed` это значит —
    /// квитанция уничтожена, оболочка увидит `DroppedWithoutConfirmation`).
    ReceiverGone,
}

#[derive(Default)]
struct MailboxState {
    events: Vec<StampedEvent>,
    stop_requested: bool,
    /// Жива ли принимающая сторона. Сбрасывается в `Drop` приёмника.
    receiver_alive: bool,
}

struct Shared {
    state: Mutex<MailboxState>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, MailboxState> {
        // Состояние — простой список; даже если держатель замка паниковал, структура
        // остаётся целой, а отказывать маршрутизатору из-за чужой паники нельзя.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Создаёт ящик: сторона маршрутизатора (клонируется) и сторона потока рендера.
pub fn render_mailbox() -> (RenderInbox, RenderInboxReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(MailboxState {
            receiver_alive: true,
            ..MailboxState::default()
        }),
        wake: Condvar::new(),
    });
    (
        RenderInbox {
            shared: Arc::clone(&shared),
        },
        RenderInboxReceiver { shared },
    )
}

/// Сторона отправителя. Дёшево клонируется, потокобезопасна; методы не блокируют.
#[derive(Clone)]
pub struct RenderInbox {
    shared: Arc<Shared>,
}

impl RenderInbox {
    /// Передаёт потоку новую surface.
    pub fn surface_created(&self, surface: NativeSurface) -> InboxDelivery {
        self.push(InboxEvent::SurfaceCreated(surface))
    }

    /// Сообщает размер surface данного поколения.
    pub fn surface_changed(&self, generation: SurfaceGeneration, size: SurfaceSize) -> InboxDelivery {
        self.push(InboxEvent::SurfaceChanged { generation, size })
    }

    /// Передаёт квитанцию освобождения. Если поток уже завершён, квитанция уничтожается здесь
    /// же — ожидающая сторона сразу получает `DroppedWithoutConfirmation`, а не ждёт таймаут.
    pub fn surface_destroyed(&self, request: SurfaceReleaseRequest) -> InboxDelivery {
        self.push(InboxEvent::SurfaceDestroyed(request))
    }

    /// Приостанавливает показ.
    pub fn paused(&self) -> InboxDelivery {
        self.push(InboxEvent::Paused)
    }

    /// Возобновляет показ.
    pub fn resumed(&self) -> InboxDelivery {
        self.push(InboxEvent::Resumed)
    }

    /// Просит поток завершиться (после обработки уже лежащих событий).
    pub fn request_stop(&self) {
        self.shared.lock().stop_requested = true;
        self.shared.wake.notify_all();
    }

    fn push(&self, event: InboxEvent) -> InboxDelivery {
        let rejected = {
            let mut state = self.shared.lock();
            if state.receiver_alive {
                enqueue_coalescing(&mut state.events, event);
                None
            } else {
                Some(event)
            }
        };
        match rejected {
            None => {
                self.shared.wake.notify_all();
                InboxDelivery::Delivered
            }
            Some(event) => {
                // Уничтожаем вне замка: Drop квитанции/surface не должен идти под мьютексом.
                drop(event);
                InboxDelivery::ReceiverGone
            }
        }
    }
}

/// Кладёт событие в конец списка, схлопывая то, что не несёт новой информации.
fn enqueue_coalescing(events: &mut Vec<StampedEvent>, event: InboxEvent) {
    let enqueued_at = Instant::now();
    match (events.last_mut(), &event) {
        // Подряд два `changed` одного поколения: нужен только последний размер.
        (
            Some(StampedEvent {
                event: InboxEvent::SurfaceChanged { generation, size },
                ..
            }),
            InboxEvent::SurfaceChanged {
                generation: new_generation,
                size: new_size,
            },
        ) if generation == new_generation => {
            *size = *new_size;
        }
        // Пауза/возобновление УСТАНАВЛИВАЮТ состояние (идемпотентно), поэтому два подряд идущих
        // события этого рода равны одному последнему. Взаимно отменять пару `Resumed`+`Paused`
        // нельзя: ящик не знает, на паузе ли уже поток (он стартует НЕ на паузе), и потерял
        // бы итоговую паузу — показ шёл бы дальше при выключенном экране.
        (
            Some(StampedEvent {
                event: last_event @ (InboxEvent::Paused | InboxEvent::Resumed),
                enqueued_at: last_enqueued_at,
            }),
            InboxEvent::Paused | InboxEvent::Resumed,
        ) => {
            *last_event = event;
            *last_enqueued_at = enqueued_at;
        }
        _ => events.push(StampedEvent { event, enqueued_at }),
    }
}

/// Сторона потока рендера. Единственная; при `Drop` ящик закрывается, а всё недоставленное
/// уничтожается (квитанции — с `DroppedWithoutConfirmation`).
pub struct RenderInboxReceiver {
    shared: Arc<Shared>,
}

impl RenderInboxReceiver {
    /// Забирает накопленное, не ожидая.
    pub fn take_pending(&self) -> InboxBatch {
        let mut state = self.shared.lock();
        Self::take_locked(&mut state)
    }

    /// Лежит ли в ящике уничтожение surface (`SurfaceDestroyed`), которое поток ещё не забрал.
    /// Не забирает событий. Поток спрашивает это перед долгим ожиданием GPU (acquire, пересоздание
    /// цепочки): оболочка ждёт квитанцию ограниченное время, и ожидание, начатое при уже лежащем
    /// запросе, съело бы бюджет освобождения.
    pub fn has_pending_surface_destroyed(&self) -> bool {
        self.shared
            .lock()
            .events
            .iter()
            .any(|stamped| matches!(stamped.event, InboxEvent::SurfaceDestroyed(_)))
    }

    /// Ждёт, пока в ящике появится событие или остановка, но не дольше `timeout`
    /// (`None` — без предела). Возвращает накопленное; пустой пакет означает таймаут.
    pub fn wait_for_events(&self, timeout: Option<Duration>) -> InboxBatch {
        let deadline = timeout.map(|limit| Instant::now() + limit);
        let mut state = self.shared.lock();
        // Цикл защищает от ложных пробуждений Condvar.
        while state.events.is_empty() && !state.stop_requested {
            state = match deadline {
                None => self.shared.wake.wait(state).unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    self.shared
                        .wake
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
        Self::take_locked(&mut state)
    }

    fn take_locked(state: &mut MailboxState) -> InboxBatch {
        InboxBatch {
            events: mem::take(&mut state.events),
            stop_requested: state.stop_requested,
        }
    }
}

impl Drop for RenderInboxReceiver {
    fn drop(&mut self) {
        let leftover = {
            let mut state = self.shared.lock();
            state.receiver_alive = false;
            mem::take(&mut state.events)
        };
        // Вне замка: уничтожение квитанций будит оболочку с `DroppedWithoutConfirmation`.
        drop(leftover);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::thread;

    use shell_contract::surface::{PlatformWindow, SurfaceReleaseOutcome, surface_release_handoff};

    use super::*;

    struct FakeWindow;

    impl PlatformWindow for FakeWindow {
        fn raw_window(&self) -> NonNull<c_void> {
            NonNull::dangling()
        }
    }

    fn surface(generation: u64) -> NativeSurface {
        NativeSurface::new(SurfaceGeneration::new(generation), Box::new(FakeWindow))
    }

    const SIZE: SurfaceSize = SurfaceSize {
        width_px: 1440,
        height_px: 3216,
    };

    #[test]
    fn events_arrive_in_order() {
        let (inbox, receiver) = render_mailbox();
        let (request, _waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        inbox.surface_created(surface(1));
        inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
        inbox.surface_destroyed(request);

        let batch = receiver.take_pending();
        let kinds: Vec<&str> = batch
            .events
            .iter()
            .map(|stamped| match stamped.event {
                InboxEvent::SurfaceCreated(_) => "created",
                InboxEvent::SurfaceChanged { .. } => "changed",
                InboxEvent::SurfaceDestroyed(_) => "destroyed",
                InboxEvent::Paused => "paused",
                InboxEvent::Resumed => "resumed",
            })
            .collect();
        assert_eq!(kinds, ["created", "changed", "destroyed"]);
        assert!(!batch.stop_requested);
        assert!(receiver.take_pending().events.is_empty(), "забранное не возвращается");
    }

    #[test]
    fn consecutive_changes_keep_only_last_size() {
        let (inbox, receiver) = render_mailbox();
        let generation = SurfaceGeneration::new(1);
        for width_px in 100..110 {
            inbox.surface_changed(
                generation,
                SurfaceSize {
                    width_px,
                    height_px: 50,
                },
            );
        }
        let batch = receiver.take_pending();
        assert_eq!(batch.events.len(), 1);
        assert!(matches!(
            batch.events[0].event,
            InboxEvent::SurfaceChanged {
                size: SurfaceSize { width_px: 109, .. },
                ..
            }
        ));
    }

    #[test]
    fn changes_of_different_generations_are_not_merged() {
        let (inbox, receiver) = render_mailbox();
        inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
        inbox.surface_changed(SurfaceGeneration::new(2), SIZE);
        assert_eq!(receiver.take_pending().events.len(), 2);
    }

    #[test]
    fn consecutive_pause_events_collapse_to_the_last_one_never_to_nothing() {
        let kinds_after = |sequence: &[bool]| {
            let (inbox, receiver) = render_mailbox();
            for &pause in sequence {
                if pause {
                    inbox.paused();
                } else {
                    inbox.resumed();
                }
            }
            receiver
                .take_pending()
                .events
                .iter()
                .map(|stamped| matches!(stamped.event, InboxEvent::Paused))
                .collect::<Vec<bool>>()
        };
        // Resumed, Paused: итоговая пауза обязана дойти до потока (он мог быть не на паузе).
        assert_eq!(kinds_after(&[false, true]), [true]);
        assert_eq!(kinds_after(&[true, false]), [false]);
        assert_eq!(kinds_after(&[true, true]), [true]);
        assert_eq!(kinds_after(&[true, false, true, false, true]), [true]);
    }

    #[test]
    fn pending_surface_destroy_is_visible_without_consuming_it() {
        let (inbox, receiver) = render_mailbox();
        assert!(!receiver.has_pending_surface_destroyed(), "пустой ящик");
        inbox.surface_changed(SurfaceGeneration::new(1), SIZE);
        inbox.paused();
        assert!(!receiver.has_pending_surface_destroyed(), "другие события не в счёт");
        let (request, _waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        inbox.surface_destroyed(request);
        assert!(receiver.has_pending_surface_destroyed());
        assert!(receiver.has_pending_surface_destroyed(), "проверка ничего не забирает");
        assert_eq!(receiver.take_pending().events.len(), 3);
        assert!(
            !receiver.has_pending_surface_destroyed(),
            "после выдачи события в ящике нет"
        );
    }

    #[test]
    fn destroy_after_receiver_gone_wakes_waiter_without_timeout() {
        let (inbox, receiver) = render_mailbox();
        drop(receiver);
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(7));
        assert_eq!(inbox.surface_destroyed(request), InboxDelivery::ReceiverGone);
        // Огромный таймаут: тест пройдёт мгновенно только если запрос реально уничтожен.
        assert_eq!(
            waiter.wait(Duration::from_secs(3600)),
            SurfaceReleaseOutcome::DroppedWithoutConfirmation
        );
    }

    #[test]
    fn dropping_receiver_with_pending_request_wakes_waiter() {
        let (inbox, receiver) = render_mailbox();
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(7));
        assert_eq!(inbox.surface_destroyed(request), InboxDelivery::Delivered);
        drop(receiver);
        assert_eq!(
            waiter.wait(Duration::from_secs(3600)),
            SurfaceReleaseOutcome::DroppedWithoutConfirmation
        );
    }

    #[test]
    fn wait_times_out_with_empty_batch() {
        let (_inbox, receiver) = render_mailbox();
        let started = Instant::now();
        let batch = receiver.wait_for_events(Some(Duration::from_millis(30)));
        assert!(batch.events.is_empty() && !batch.stop_requested);
        assert!(started.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn sleeping_receiver_wakes_on_event_from_other_thread() {
        let (inbox, receiver) = render_mailbox();
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            inbox.surface_created(surface(1));
        });
        let started = Instant::now();
        let batch = receiver.wait_for_events(None);
        assert_eq!(batch.events.len(), 1);
        assert!(started.elapsed() < Duration::from_secs(5));
        sender.join().expect("отправитель");
    }

    #[test]
    fn stop_request_wakes_sleeping_receiver() {
        let (inbox, receiver) = render_mailbox();
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            inbox.request_stop();
        });
        let batch = receiver.wait_for_events(None);
        assert!(batch.stop_requested && batch.events.is_empty());
        stopper.join().expect("останавливающий");
    }

    #[test]
    fn events_queued_before_stop_are_still_delivered() {
        let (inbox, receiver) = render_mailbox();
        inbox.surface_created(surface(1));
        inbox.request_stop();
        let batch = receiver.take_pending();
        assert!(batch.stop_requested);
        assert_eq!(batch.events.len(), 1);
    }
}
