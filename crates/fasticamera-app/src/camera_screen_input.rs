//! Канал «вход экрана»: ограниченная очередь от потока маршрутизатора к источнику UI на потоке рендера (сессия 12).
//!
//! # Зачем
//! Касания, клавиша громкости и ориентация приходят на поток маршрутизатора, а экран камеры живёт на потоке
//! рендера (он считает анимации от времени показа кадра). Между потоками — только эта очередь:
//! * отправка — `try_send`, поток маршрутизатора НИКОГДА не ждёт (правило проекта: горячий путь без блокировок);
//! * очередь ограничена (`camera_screen.input_queue_capacity`), переполнение — явный счётчик, а не тишина.
//!
//! # Переполнение и жесты
//! Потерянное касание ломает жест: экран увидел бы `Began`, не увидел `Ended`, и палец «залип» бы в жесте.
//! Поэтому после потери КАСАНИЯ отправитель «должен» экрану разрыв потока ([`ScreenTouch::Gap`]) и доставляет
//! его первым, как только в очереди появится место; пока разрыв не доставлен, новые образцы касаний
//! отбрасываются (со счётчиком), чтобы разрыв не оказался позади образцов, которые идут после дыры.
//! Потеря не-касаний (клавиша, ориентация, итог снимка) жесты не ломает — только счётчик.
//!
//! # Владельцы
//! * [`ScreenInputSender`] — у потока маршрутизатора, один писатель (держит долг разрыва);
//! * [`ScreenInputInjector`] — дешёвый клон для других писателей действий (фейк-сервис снимка), без долга разрыва;
//! * [`ScreenInputReceiver`] — у источника UI на потоке рендера.
//!
//! # Жив ли получатель
//! Рендер вправе снять источник сам (паника источника, отказ создания рендера UI): тогда получатель
//! уничтожается, а сервис экрана об этом иначе не узнал бы. Получатель держит «маячок» (`Arc<()>`), писатели —
//! слабые ссылки на него; [`ScreenInputSender::receiver_state`] отвечает без отправки сообщения.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Weak};

use camera_screen::{KeyPhase as ScreenKeyPhase, ScreenAction, ScreenTouch};
use shell_contract::hardware_key::{HardwareKey, HardwareKeyEvent, KeyPhase as ShellKeyPhase};
use shell_contract::system_gesture::SystemGestureZones;
use shell_contract::touch::TouchSample;

/// Клавиша громкости → действие экрана. Первое нажатие — `Pressed` (затвор), автоповтор удержания — `Repeated`
/// (в v1 без действия), отпускание — `Released`. Обе клавиши громкости равнозначны (`scope-v1.md`).
pub fn volume_key_action(event: HardwareKeyEvent) -> ScreenAction {
    let phase = match (event.key, event.phase) {
        (HardwareKey::VolumeUp | HardwareKey::VolumeDown, ShellKeyPhase::Released) => ScreenKeyPhase::Released,
        (HardwareKey::VolumeUp | HardwareKey::VolumeDown, ShellKeyPhase::Pressed) if event.is_initial_press() => {
            ScreenKeyPhase::Pressed
        }
        (HardwareKey::VolumeUp | HardwareKey::VolumeDown, ShellKeyPhase::Pressed) => ScreenKeyPhase::Repeated,
    };
    ScreenAction::VolumeKey { phase, at: event.time }
}

/// Одно сообщение входа экрана. С сессии 13 не `Copy`: действие несёт факты камеры с общим каталогом зума.
#[derive(Debug, Clone, PartialEq)]
pub enum ScreenInput {
    /// Касание или разрыв потока касаний.
    Touch(ScreenTouch),
    /// Действие над экраном (клавиша, ориентация, итог снимка…).
    Action(ScreenAction),
    /// Новые зоны обязательных системных жестов: настройка фильтра входа экрана, не действие редьюсера
    /// (зоны не часть состояния экрана — они лишь решают, какие касания до него доходят).
    SystemGestureZones(SystemGestureZones),
}

/// Итог отправки одного сообщения. Не `bool`: «в очереди», «отброшено из-за переполнения» и «получателя нет» —
/// разные ситуации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDelivery {
    /// Сообщение лежит в очереди.
    Queued,
    /// Очередь полна (или впереди недоставленный разрыв): сообщение потеряно и посчитано.
    Overflowed,
    /// Источник UI уничтожен (экран снят): слать некому.
    ReceiverGone,
}

/// Жив ли получатель канала (источник UI на потоке рендера).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverState {
    /// Получатель существует (в том числе пока лежит в ящике рендера и ещё не применён).
    Alive,
    /// Получатель уничтожен: рендер снял источник, слать некому.
    Gone,
}

/// Итог попытки досрочно погасить долг разрыва (по тику маршрутизатора, без нового касания).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapFlush {
    /// Долга не было: ничего не отправлено.
    NotOwed,
    /// Разрыв доставлен в очередь, долг погашен.
    Queued,
    /// Очередь всё ещё полна: долг остался, повторим на следующем тике.
    QueueFull,
    /// Получателя нет: долг не имеет смысла.
    ReceiverGone,
}

/// Накопительные счётчики канала (общие для всех писателей и читателя).
#[derive(Debug, Default)]
struct InputCounterCells {
    queued_touches: AtomicU64,
    queued_actions: AtomicU64,
    overflowed_touches: AtomicU64,
    overflowed_actions: AtomicU64,
    gaps_queued: AtomicU64,
    receiver_gone: AtomicU64,
    received: AtomicU64,
}

/// Снимок счётчиков канала входа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenInputStats {
    /// Образцы касаний, попавшие в очередь.
    pub queued_touches: u64,
    /// Действия, попавшие в очередь.
    pub queued_actions: u64,
    /// Касания (образцы и разрывы), потерянные из-за переполнения.
    pub overflowed_touches: u64,
    /// Действия, потерянные из-за переполнения.
    pub overflowed_actions: u64,
    /// Разрывы, добавленные отправителем после потери касаний (жесты экрана сброшены).
    pub gaps_queued: u64,
    /// Сообщения, отправленные уничтоженному получателю.
    pub receiver_gone: u64,
    /// Сообщения, забранные источником UI.
    pub received: u64,
}

impl InputCounterCells {
    fn snapshot(&self) -> ScreenInputStats {
        ScreenInputStats {
            queued_touches: self.queued_touches.load(Ordering::Relaxed),
            queued_actions: self.queued_actions.load(Ordering::Relaxed),
            overflowed_touches: self.overflowed_touches.load(Ordering::Relaxed),
            overflowed_actions: self.overflowed_actions.load(Ordering::Relaxed),
            gaps_queued: self.gaps_queued.load(Ordering::Relaxed),
            receiver_gone: self.receiver_gone.load(Ordering::Relaxed),
            received: self.received.load(Ordering::Relaxed),
        }
    }
}

/// Создаёт канал входа экрана заданной ёмкости. Ёмкость уже проверена конфигом (≥ 1).
pub fn screen_input_channel(capacity: usize) -> (ScreenInputSender, ScreenInputReceiver) {
    let (sender, receiver) = sync_channel(capacity);
    let counters = Arc::new(InputCounterCells::default());
    let lifeline = Arc::new(());
    (
        ScreenInputSender {
            injector: ScreenInputInjector {
                sender,
                counters: Arc::clone(&counters),
                receiver_lifeline: Arc::downgrade(&lifeline),
            },
            gap_owed: false,
        },
        ScreenInputReceiver {
            receiver,
            counters,
            _lifeline: lifeline,
        },
    )
}

/// Писатель действий без права на касания: клон для сервисов, отвечающих экрану (фейк-снимок).
#[derive(Clone)]
pub struct ScreenInputInjector {
    sender: SyncSender<ScreenInput>,
    counters: Arc<InputCounterCells>,
    /// Слабая ссылка на маячок получателя: не оживает, когда получатель уничтожен.
    receiver_lifeline: Weak<()>,
}

impl ScreenInputInjector {
    /// Кладёт действие в очередь без ожидания.
    pub fn inject(&self, action: ScreenAction) -> InputDelivery {
        self.send_counted_as_action(ScreenInput::Action(action))
    }

    /// Кладёт в очередь образец касания (для сценариев, которые водят пальцем по экрану сами).
    ///
    /// В отличие от [`ScreenInputSender::send_touch`] долга разрыва не ведёт: у сценария нет права «чинить» поток
    /// касаний, поэтому потеря образца (`Overflowed`) — повод провалить сценарий, а не молча склеить жест. Касания
    /// живого пальца идут тем же каналом: пока идёт сценарий, экран трогать нельзя (правило всех smoke-сценариев).
    pub fn inject_touch(&self, sample: TouchSample) -> InputDelivery {
        let delivery = self.try_send(ScreenInput::Touch(ScreenTouch::Sample(sample)));
        match delivery {
            InputDelivery::Queued => &self.counters.queued_touches,
            InputDelivery::Overflowed => &self.counters.overflowed_touches,
            InputDelivery::ReceiverGone => &self.counters.receiver_gone,
        }
        .fetch_add(1, Ordering::Relaxed);
        delivery
    }

    /// Кладёт в очередь зоны системных жестов. В счётчиках идёт вместе с действиями: это такое же редкое
    /// не-касание, потеря которого жесты не ломает (зоны остаются прежними до следующего сообщения платформы).
    pub fn inject_system_gesture_zones(&self, zones: SystemGestureZones) -> InputDelivery {
        self.send_counted_as_action(ScreenInput::SystemGestureZones(zones))
    }

    /// Отправка не-касания с учётом в счётчиках действий.
    fn send_counted_as_action(&self, input: ScreenInput) -> InputDelivery {
        let delivery = self.try_send(input);
        match delivery {
            InputDelivery::Queued => &self.counters.queued_actions,
            InputDelivery::Overflowed => &self.counters.overflowed_actions,
            InputDelivery::ReceiverGone => &self.counters.receiver_gone,
        }
        .fetch_add(1, Ordering::Relaxed);
        delivery
    }

    fn try_send(&self, input: ScreenInput) -> InputDelivery {
        match self.sender.try_send(input) {
            Ok(()) => InputDelivery::Queued,
            Err(TrySendError::Full(_)) => InputDelivery::Overflowed,
            Err(TrySendError::Disconnected(_)) => InputDelivery::ReceiverGone,
        }
    }

    /// Счётчики канала (общие со всеми концами).
    pub fn stats(&self) -> ScreenInputStats {
        self.counters.snapshot()
    }

    /// Жив ли получатель. Ничего не отправляет и счётчики не трогает.
    pub fn receiver_state(&self) -> ReceiverState {
        if self.receiver_lifeline.strong_count() > 0 {
            ReceiverState::Alive
        } else {
            ReceiverState::Gone
        }
    }
}

/// Единственный писатель касаний (поток маршрутизатора). Хранит «долг» разрыва после потери касания.
pub struct ScreenInputSender {
    injector: ScreenInputInjector,
    /// После потери касания экрану нужно сообщить о разрыве, прежде чем слать новые образцы.
    gap_owed: bool,
}

impl ScreenInputSender {
    /// Клон для писателей действий.
    pub fn injector(&self) -> ScreenInputInjector {
        self.injector.clone()
    }

    /// Счётчики канала.
    pub fn stats(&self) -> ScreenInputStats {
        self.injector.stats()
    }

    /// Касание или разрыв потока касаний. Образцы, потерянные из-за переполнения, оставляют долг разрыва.
    pub fn send_touch(&mut self, touch: ScreenTouch) -> InputDelivery {
        // Сначала погасить долг: разрыв обязан идти раньше любых образцов после дыры.
        match self.settle_gap_debt() {
            GapFlush::NotOwed | GapFlush::Queued => {}
            GapFlush::QueueFull => {
                // Разрыв всё ещё не влез; образец за ним нельзя пускать вперёд — теряем и его.
                self.injector
                    .counters
                    .overflowed_touches
                    .fetch_add(1, Ordering::Relaxed);
                return InputDelivery::Overflowed;
            }
            GapFlush::ReceiverGone => {
                self.injector.counters.receiver_gone.fetch_add(1, Ordering::Relaxed);
                return InputDelivery::ReceiverGone;
            }
        }
        let counters = &self.injector.counters;
        let delivery = self.injector.try_send(ScreenInput::Touch(touch));
        match delivery {
            InputDelivery::Queued => {
                counters.queued_touches.fetch_add(1, Ordering::Relaxed);
            }
            InputDelivery::Overflowed => {
                counters.overflowed_touches.fetch_add(1, Ordering::Relaxed);
                // Потерян образец или сам разрыв — в обоих случаях жесты экрана надо сбросить.
                self.gap_owed = true;
            }
            InputDelivery::ReceiverGone => {
                counters.receiver_gone.fetch_add(1, Ordering::Relaxed);
            }
        }
        delivery
    }

    /// Досылает долг разрыва БЕЗ нового касания (зовёт тик маршрутизатора).
    ///
    /// Зачем: если потерян `Ended` и новых касаний нет, экран считал бы палец нажатым, а его таймер мог бы
    /// выстрелить ложный долгий тап (AE/AF Lock). Долг без этого погашался бы только следующим касанием.
    pub fn flush_owed_gap(&mut self) -> GapFlush {
        self.settle_gap_debt()
    }

    /// Пытается положить в очередь разрыв, если он должен; единственное место, где долг гасится.
    fn settle_gap_debt(&mut self) -> GapFlush {
        if !self.gap_owed {
            return GapFlush::NotOwed;
        }
        match self.injector.try_send(ScreenInput::Touch(ScreenTouch::Gap)) {
            InputDelivery::Queued => {
                self.gap_owed = false;
                self.injector.counters.gaps_queued.fetch_add(1, Ordering::Relaxed);
                GapFlush::Queued
            }
            InputDelivery::Overflowed => GapFlush::QueueFull,
            InputDelivery::ReceiverGone => GapFlush::ReceiverGone,
        }
    }

    /// Жив ли получатель (источник UI на потоке рендера).
    pub fn receiver_state(&self) -> ReceiverState {
        self.injector.receiver_state()
    }

    /// Действие над экраном (клавиша, ориентация…).
    pub fn send_action(&self, action: ScreenAction) -> InputDelivery {
        self.injector.inject(action)
    }

    /// Зоны системных жестов (редкое не-касание, без ожидания).
    pub fn send_system_gesture_zones(&self, zones: SystemGestureZones) -> InputDelivery {
        self.injector.inject_system_gesture_zones(zones)
    }

    /// Должен ли отправитель экрану разрыв (для тестов и диагностики).
    pub fn owes_gap(&self) -> bool {
        self.gap_owed
    }
}

/// Читатель канала на потоке рендера.
pub struct ScreenInputReceiver {
    receiver: Receiver<ScreenInput>,
    counters: Arc<InputCounterCells>,
    /// Маячок «получатель жив»: пока получатель существует, слабые ссылки писателей оживают.
    _lifeline: Arc<()>,
}

impl ScreenInputReceiver {
    /// Забирает следующее сообщение без ожидания; `None` — очередь пуста (или все писатели ушли).
    pub fn try_next(&mut self) -> Option<ScreenInput> {
        match self.receiver.try_recv() {
            Ok(input) => {
                self.counters.received.fetch_add(1, Ordering::Relaxed);
                Some(input)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    /// Счётчики канала.
    pub fn stats(&self) -> ScreenInputStats {
        self.counters.snapshot()
    }
}

#[cfg(test)]
#[path = "camera_screen_input_tests.rs"]
mod tests;
