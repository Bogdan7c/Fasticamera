//! Ограниченная очередь событий оболочки: главный поток Android → поток маршрутизации Rust.
//!
//! Правила (README, сессия 02, «Что сделать», п. 3):
//! * отправитель **никогда не ждёт** — главный поток Android не блокируется очередью;
//! * одна FIFO на все события, поэтому порядок сохраняется;
//! * касаний в очереди не больше `touch_capacity`. Лишнее касание выбрасывается и считается,
//!   а перед следующим доставленным касанием приходит [`ShellEvent::TouchGap`] с числом
//!   потерь, чтобы распознаватели жестов честно сбросились;
//! * для остальных («критичных») событий всегда оставлено `critical_capacity` мест сверх
//!   касаний. Касания их не вытесняют. Если критичных событий накопилось больше (потребитель
//!   завис), лишние теряются со счётчиком. Квитанция `SurfaceDestroyed` при этом
//!   уничтожается, и ожидающая сторона сразу узнаёт, что подтверждения не будет.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::event::ShellEvent;

/// Ёмкости очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShellQueueConfig {
    /// Сколько касаний может ждать в очереди. По умолчанию 256: при опросе экрана 120 Гц,
    /// двух пальцах и промежуточных точках это больше секунды касаний — дольше потребитель
    /// стоять не должен.
    pub touch_capacity: usize,
    /// Сколько мест гарантировано остальным событиям. По умолчанию 64: событий жизненного
    /// цикла и сервисов приходит единицы в секунду.
    pub critical_capacity: usize,
}

impl Default for ShellQueueConfig {
    fn default() -> Self {
        Self {
            touch_capacity: 256,
            critical_capacity: 64,
        }
    }
}

/// Неверная конфигурация очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ShellQueueConfigError {
    #[error("ёмкости очереди событий должны быть больше нуля (касания {touch}, остальные {critical})")]
    ZeroCapacity { touch: usize, critical: usize },
    #[error("ёмкость очереди событий слишком велика (касания {touch} + остальные {critical})")]
    CapacityOverflow { touch: usize, critical: usize },
}

/// Итог отправки события.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Delivered,
    /// Касание выброшено: в очереди уже `touch_capacity` касаний.
    TouchDropped,
    /// Критичное событие потеряно: очередь полна (потребитель не успевает).
    CriticalLost,
    /// Получателя больше нет (поток маршрутизации завершился).
    ReceiverGone,
}

/// Счётчики очереди с начала её жизни.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ShellQueueStats {
    pub delivered: u64,
    pub touch_samples_dropped: u64,
    pub critical_events_lost: u64,
}

/// Почему не получено событие.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveError {
    /// За отведённое время событий не было.
    Timeout,
    /// Все отправители уничтожены — событий больше не будет.
    SendersGone,
}

#[derive(Debug)]
struct SharedState {
    touch_capacity: usize,
    /// Сколько касаний сейчас в канале (зарезервировано отправителем, не забрано получателем).
    touches_in_queue: AtomicUsize,
    /// Сколько касаний выброшено с момента последнего доставленного `TouchGap`.
    undelivered_gap: AtomicU64,
    delivered: AtomicU64,
    touch_samples_dropped: AtomicU64,
    critical_events_lost: AtomicU64,
}

impl SharedState {
    fn stats(&self) -> ShellQueueStats {
        ShellQueueStats {
            delivered: self.delivered.load(Ordering::Relaxed),
            touch_samples_dropped: self.touch_samples_dropped.load(Ordering::Relaxed),
            critical_events_lost: self.critical_events_lost.load(Ordering::Relaxed),
        }
    }
}

/// Отправляющая сторона (у оболочки). Клонируется для разных колбэков и потоков датчиков.
#[derive(Debug, Clone)]
pub struct ShellEventSender {
    channel: SyncSender<ShellEvent>,
    shared: Arc<SharedState>,
}

/// Принимающая сторона (у маршрутизатора `fasticamera-app`).
#[derive(Debug)]
pub struct ShellEventReceiver {
    channel: Receiver<ShellEvent>,
    shared: Arc<SharedState>,
}

/// Создаёт очередь событий оболочки.
pub fn shell_event_channel(
    config: ShellQueueConfig,
) -> Result<(ShellEventSender, ShellEventReceiver), ShellQueueConfigError> {
    let ShellQueueConfig {
        touch_capacity: touch,
        critical_capacity: critical,
    } = config;
    if touch == 0 || critical == 0 {
        return Err(ShellQueueConfigError::ZeroCapacity { touch, critical });
    }
    let total = touch
        .checked_add(critical)
        .ok_or(ShellQueueConfigError::CapacityOverflow { touch, critical })?;
    // Общая ёмкость канала = касания + запас критичных: касания ограничены своим счётчиком,
    // поэтому `critical` мест всегда остаются за остальными событиями.
    let (channel_sender, channel_receiver) = sync_channel(total);
    let shared = Arc::new(SharedState {
        touch_capacity: touch,
        touches_in_queue: AtomicUsize::new(0),
        undelivered_gap: AtomicU64::new(0),
        delivered: AtomicU64::new(0),
        touch_samples_dropped: AtomicU64::new(0),
        critical_events_lost: AtomicU64::new(0),
    });
    Ok((
        ShellEventSender {
            channel: channel_sender,
            shared: Arc::clone(&shared),
        },
        ShellEventReceiver {
            channel: channel_receiver,
            shared,
        },
    ))
}

impl ShellEventSender {
    /// Отправляет событие, никогда не блокируясь. Касания шлёт один поток (главный поток
    /// Android); учёт мест и долга дыры при этом всё равно атомарный и выдержит несколько.
    pub fn send(&self, event: ShellEvent) -> SendOutcome {
        match event {
            ShellEvent::Touch(_) => self.send_touch(event),
            critical => self.send_critical(critical),
        }
    }

    pub fn stats(&self) -> ShellQueueStats {
        self.shared.stats()
    }

    fn send_critical(&self, event: ShellEvent) -> SendOutcome {
        match self.channel.try_send(event) {
            Ok(()) => {
                self.shared.delivered.fetch_add(1, Ordering::Relaxed);
                SendOutcome::Delivered
            }
            Err(TrySendError::Full(_lost)) => {
                self.shared.critical_events_lost.fetch_add(1, Ordering::Relaxed);
                SendOutcome::CriticalLost
            }
            Err(TrySendError::Disconnected(_)) => SendOutcome::ReceiverGone,
        }
    }

    fn send_touch(&self, touch: ShellEvent) -> SendOutcome {
        // Резерв места касания: fetch_add, затем проверка. Так два отправителя не превысят ёмкость.
        let already_queued = self.shared.touches_in_queue.fetch_add(1, Ordering::AcqRel);
        if already_queued >= self.shared.touch_capacity {
            self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
            return self.count_dropped_touch();
        }
        // Место есть — сначала долг о дыре, чтобы она пришла раньше этого касания. Если дыру
        // доставить нельзя, касание тоже не отправляем: касание после невидимой дыры сломало бы
        // жест, а так долг просто растёт и уйдёт одним событием позже.
        match self.deliver_pending_gap() {
            SendOutcome::Delivered => {}
            SendOutcome::ReceiverGone => {
                self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
                return SendOutcome::ReceiverGone;
            }
            SendOutcome::TouchDropped | SendOutcome::CriticalLost => {
                self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
                return self.count_dropped_touch();
            }
        }
        match self.channel.try_send(touch) {
            Ok(()) => {
                self.shared.delivered.fetch_add(1, Ordering::Relaxed);
                SendOutcome::Delivered
            }
            Err(TrySendError::Full(_)) => {
                // Сюда попадаем, только если критичные события заняли весь канал.
                self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
                self.count_dropped_touch()
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
                SendOutcome::ReceiverGone
            }
        }
    }

    fn count_dropped_touch(&self) -> SendOutcome {
        self.shared.touch_samples_dropped.fetch_add(1, Ordering::Relaxed);
        self.shared.undelivered_gap.fetch_add(1, Ordering::AcqRel);
        SendOutcome::TouchDropped
    }

    /// Доставляет `TouchGap`, если касания выбрасывались. Не получилось (очередь полна) —
    /// долг остаётся и уйдёт перед следующим касанием.
    fn deliver_pending_gap(&self) -> SendOutcome {
        // Забираем весь долг атомарно (swap): два отправителя не доставят одну и ту же дыру
        // дважды и не уведут счётчик в переполнение. Не доставили — возвращаем долг обратно.
        let dropped_samples = self.shared.undelivered_gap.swap(0, Ordering::AcqRel);
        if dropped_samples == 0 {
            return SendOutcome::Delivered;
        }
        match self.channel.try_send(ShellEvent::TouchGap { dropped_samples }) {
            Ok(()) => {
                self.shared.delivered.fetch_add(1, Ordering::Relaxed);
                SendOutcome::Delivered
            }
            Err(TrySendError::Full(_)) => {
                self.shared.undelivered_gap.fetch_add(dropped_samples, Ordering::AcqRel);
                SendOutcome::CriticalLost
            }
            Err(TrySendError::Disconnected(_)) => {
                self.shared.undelivered_gap.fetch_add(dropped_samples, Ordering::AcqRel);
                SendOutcome::ReceiverGone
            }
        }
    }
}

impl ShellEventReceiver {
    /// Ждёт событие не дольше `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<ShellEvent, ReceiveError> {
        match self.channel.recv_timeout(timeout) {
            Ok(event) => Ok(self.account_received(event)),
            Err(RecvTimeoutError::Timeout) => Err(ReceiveError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ReceiveError::SendersGone),
        }
    }

    /// Забирает событие, если оно уже есть.
    pub fn try_recv(&self) -> Result<ShellEvent, ReceiveError> {
        match self.channel.try_recv() {
            Ok(event) => Ok(self.account_received(event)),
            Err(TryRecvError::Empty) => Err(ReceiveError::Timeout),
            Err(TryRecvError::Disconnected) => Err(ReceiveError::SendersGone),
        }
    }

    pub fn stats(&self) -> ShellQueueStats {
        self.shared.stats()
    }

    fn account_received(&self, event: ShellEvent) -> ShellEvent {
        if matches!(event, ShellEvent::Touch(_)) {
            self.shared.touches_in_queue.fetch_sub(1, Ordering::AcqRel);
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use super::*;
    use crate::clock::MonotonicNanos;
    use crate::event::ShellEventKind;
    use crate::surface::{SurfaceGeneration, SurfaceReleaseOutcome, surface_release_handoff};
    use crate::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};

    fn touch(time_ns: i64) -> ShellEvent {
        ShellEvent::Touch(TouchSample {
            pointer: PointerId::new(0),
            phase: TouchPhase::Moved,
            position: SurfacePoint { x_px: 1.0, y_px: 2.0 },
            time: MonotonicNanos::from_nanos(time_ns),
        })
    }

    fn channel(touch_capacity: usize, critical_capacity: usize) -> (ShellEventSender, ShellEventReceiver) {
        shell_event_channel(ShellQueueConfig {
            touch_capacity,
            critical_capacity,
        })
        .expect("очередь")
    }

    fn drain(receiver: &ShellEventReceiver) -> Vec<ShellEvent> {
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    }

    #[test]
    fn touch_over_capacity_is_dropped_and_counted_without_blocking() {
        const CAPACITY: usize = 4;
        let (sender, receiver) = channel(CAPACITY, 2);
        // Отправка идёт в отдельном потоке «главного потока Android»: если бы send блокировался
        // на полной очереди, поток не завершился бы, и recv_timeout ниже упал бы по таймауту.
        let (done_sender, done_receiver) = mpsc::channel();
        let main_thread = thread::spawn(move || {
            let outcomes: Vec<SendOutcome> = (0..=CAPACITY as i64).map(|index| sender.send(touch(index))).collect();
            done_sender.send((outcomes, sender.stats())).expect("итог");
        });
        let (outcomes, stats) = done_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("главный поток не должен блокироваться");
        main_thread.join().expect("поток");
        assert_eq!(outcomes[..CAPACITY], [SendOutcome::Delivered; CAPACITY]);
        assert_eq!(outcomes[CAPACITY], SendOutcome::TouchDropped);
        assert_eq!(stats.touch_samples_dropped, 1);
        assert_eq!(drain(&receiver).len(), CAPACITY);
    }

    #[test]
    fn gap_arrives_before_next_touch_after_drop() {
        let (sender, receiver) = channel(1, 2);
        assert_eq!(sender.send(touch(1)), SendOutcome::Delivered);
        assert_eq!(sender.send(touch(2)), SendOutcome::TouchDropped);
        assert_eq!(sender.send(touch(3)), SendOutcome::TouchDropped);
        // Потребитель освободил место — следующее касание идёт после дыры на 2 записи.
        assert!(matches!(receiver.try_recv(), Ok(ShellEvent::Touch(_))));
        assert_eq!(sender.send(touch(4)), SendOutcome::Delivered);
        let events = drain(&receiver);
        assert!(
            matches!(events.as_slice(), [ShellEvent::TouchGap { dropped_samples: 2 }, ShellEvent::Touch(sample)] if sample.time.as_nanos() == 4),
            "{events:?}"
        );
        // Долг погашен: следующее касание идёт без новой дыры.
        assert_eq!(sender.send(touch(5)), SendOutcome::Delivered);
        assert!(matches!(drain(&receiver).as_slice(), [ShellEvent::Touch(_)]));
    }

    #[test]
    fn gap_debt_survives_full_channel_and_is_repaid_later() {
        // Касаний до 2, всего в канале 3 места.
        let (sender, receiver) = channel(2, 1);
        assert_eq!(sender.send(touch(1)), SendOutcome::Delivered);
        assert_eq!(sender.send(touch(2)), SendOutcome::Delivered);
        assert_eq!(
            sender.send(touch(3)),
            SendOutcome::TouchDropped,
            "мест для касаний нет: долг 1"
        );
        assert_eq!(sender.send(ShellEvent::Paused), SendOutcome::Delivered);
        assert!(matches!(receiver.try_recv(), Ok(ShellEvent::Touch(_))));
        // Место касания освободилось, а свободное место канала заняло критичное событие:
        // дыру доставить некуда — касание тоже не уходит, долг возвращается и растёт до 2.
        assert_eq!(sender.send(ShellEvent::Resumed), SendOutcome::Delivered);
        assert_eq!(sender.send(touch(4)), SendOutcome::TouchDropped);
        assert_eq!(drain(&receiver).len(), 3, "касание 2, Paused, Resumed");
        assert_eq!(sender.send(touch(5)), SendOutcome::Delivered);
        let events = drain(&receiver);
        assert!(
            matches!(events.as_slice(), [ShellEvent::TouchGap { dropped_samples: 2 }, ShellEvent::Touch(sample)] if sample.time.as_nanos() == 5),
            "долг 2 (касания 3 и 4) доставлен одним событием перед касанием 5: {events:?}"
        );
        assert_eq!(sender.stats().touch_samples_dropped, 2);
    }

    #[test]
    fn touches_never_take_places_reserved_for_lifecycle() {
        let (sender, receiver) = channel(2, 2);
        for index in 0..10 {
            sender.send(touch(index));
        }
        assert_eq!(sender.send(ShellEvent::Paused), SendOutcome::Delivered);
        assert_eq!(sender.send(ShellEvent::Resumed), SendOutcome::Delivered);
        let kinds: Vec<ShellEventKind> = drain(&receiver).iter().map(ShellEvent::kind).collect();
        assert_eq!(
            kinds,
            [
                ShellEventKind::Touch,
                ShellEventKind::Touch,
                ShellEventKind::Paused,
                ShellEventKind::Resumed
            ]
        );
        assert_eq!(receiver.stats().touch_samples_dropped, 8);
        assert_eq!(receiver.stats().critical_events_lost, 0);
    }

    fn volume_press(time_ns: i64) -> ShellEvent {
        use crate::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase, KeyRepeat};
        ShellEvent::HardwareKey(HardwareKeyEvent {
            key: HardwareKey::VolumeUp,
            phase: KeyPhase::Pressed,
            repeat: KeyRepeat::Initial,
            time: MonotonicNanos::from_nanos(time_ns),
        })
    }

    #[test]
    fn hardware_key_survives_touch_flood_and_keeps_its_data() {
        let (sender, receiver) = channel(2, 2);
        for index in 0..50 {
            sender.send(touch(index));
        }
        assert_eq!(sender.send(volume_press(123)), SendOutcome::Delivered);
        let keys: Vec<_> = drain(&receiver)
            .into_iter()
            .filter_map(|event| match event {
                ShellEvent::HardwareKey(key) => Some(key),
                _ => None,
            })
            .collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].time, MonotonicNanos::from_nanos(123));
        assert_eq!(receiver.stats().critical_events_lost, 0);
    }

    #[test]
    fn hardware_key_over_critical_capacity_is_lost_and_counted_not_silent() {
        let (sender, receiver) = channel(1, 2);
        assert_eq!(sender.send(volume_press(1)), SendOutcome::Delivered);
        assert_eq!(sender.send(volume_press(2)), SendOutcome::Delivered);
        // Общая ёмкость канала = 1 + 2 = 3 места; четвёртое событие не помещается.
        assert_eq!(sender.send(volume_press(3)), SendOutcome::Delivered);
        assert_eq!(sender.send(volume_press(4)), SendOutcome::CriticalLost);
        assert_eq!(sender.stats().critical_events_lost, 1);
        assert_eq!(drain(&receiver).len(), 3);
    }

    #[test]
    fn system_gesture_zones_survive_touch_flood_and_keep_their_data() {
        use crate::system_gesture::{RawEdgeInsetsPx, ScreenEdge, SystemGestureZones};
        let zones = SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
            left: 0,
            top: 156,
            right: 0,
            bottom: 132,
        })
        .expect("корректные зоны");
        let (sender, receiver) = channel(2, 2);
        for index in 0..50 {
            sender.send(touch(index));
        }
        assert_eq!(
            sender.send(ShellEvent::SystemGestureZones(zones)),
            SendOutcome::Delivered
        );
        let received: Vec<_> = drain(&receiver)
            .into_iter()
            .filter_map(|event| match event {
                ShellEvent::SystemGestureZones(zones) => Some(zones),
                _ => None,
            })
            .collect();
        assert_eq!(received, [zones]);
        assert_eq!(received[0].thickness_px(ScreenEdge::Bottom), 132);
        assert_eq!(receiver.stats().critical_events_lost, 0);
    }

    #[test]
    fn order_of_mixed_events_is_preserved() {
        let (sender, receiver) = channel(8, 8);
        sender.send(ShellEvent::Resumed);
        sender.send(touch(1));
        sender.send(ShellEvent::Paused);
        sender.send(touch(2));
        let kinds: Vec<ShellEventKind> = drain(&receiver).iter().map(ShellEvent::kind).collect();
        assert_eq!(
            kinds,
            [
                ShellEventKind::Resumed,
                ShellEventKind::Touch,
                ShellEventKind::Paused,
                ShellEventKind::Touch
            ]
        );
    }

    #[test]
    fn lost_surface_destroyed_releases_waiter_immediately() {
        let (sender, _receiver) = channel(1, 1);
        // Критичные события могут занять и свободные места касаний: всего в канале 1 + 1.
        assert_eq!(sender.send(ShellEvent::Paused), SendOutcome::Delivered);
        assert_eq!(sender.send(ShellEvent::Resumed), SendOutcome::Delivered);
        let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
        // Очередь критичных полна (потребитель завис): событие потеряно со счётчиком,
        // а оболочка не ждёт час — квитанция уничтожена вместе с событием.
        assert_eq!(
            sender.send(ShellEvent::SurfaceDestroyed(request)),
            SendOutcome::CriticalLost
        );
        assert_eq!(
            waiter.wait(Duration::from_secs(3600)),
            SurfaceReleaseOutcome::DroppedWithoutConfirmation
        );
        assert_eq!(sender.stats().critical_events_lost, 1);
    }

    #[test]
    fn send_after_receiver_gone_reports_it() {
        let (sender, receiver) = channel(2, 2);
        drop(receiver);
        assert_eq!(sender.send(ShellEvent::Resumed), SendOutcome::ReceiverGone);
        assert_eq!(sender.send(touch(1)), SendOutcome::ReceiverGone);
        assert_eq!(sender.stats().touch_samples_dropped, 0);
    }

    #[test]
    fn receiver_sees_senders_gone() {
        let (sender, receiver) = channel(2, 2);
        drop(sender);
        assert_eq!(
            receiver.recv_timeout(Duration::from_millis(10)).err(),
            Some(ReceiveError::SendersGone)
        );
    }

    #[test]
    fn zero_capacity_is_rejected() {
        let config = ShellQueueConfig {
            touch_capacity: 0,
            critical_capacity: 4,
        };
        assert_eq!(
            shell_event_channel(config).err(),
            Some(ShellQueueConfigError::ZeroCapacity { touch: 0, critical: 4 })
        );
        let overflow = ShellQueueConfig {
            touch_capacity: usize::MAX,
            critical_capacity: 1,
        };
        assert!(matches!(
            shell_event_channel(overflow).err(),
            Some(ShellQueueConfigError::CapacityOverflow { .. })
        ));
    }
}
