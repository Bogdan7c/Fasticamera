//! Почтовый ящик потока камеры: сюда пишут все, а читает один поток `fasticamera-camera`.
//!
//! Три полосы с явными приоритетами (читаются в этом порядке; пробуждение 3A — флаг между
//! полосами 1 и 2, см. [`ThreeAWaker`]):
//! 1. **редкие события бэкенда** (отключение, ошибка устройства, закрытие сессии, свобода
//!    камеры) и результаты **разовых** запросов (триггер фокуса, кадры серии) — без лимита и
//!    никогда не теряются: их мало, а потеря превращается в зависшую камеру или тап/серию
//!    без итога;
//! 2. **команды** — не больше `queues.command_capacity`; переполнение — отказ отправителю
//!    ([`CommandRejected::QueueFull`]), команда не теряется молча;
//! 3. **частые события** (результаты кадров) — не больше `queues.capture_event_capacity`;
//!    при переполнении вытесняется **старейшее** со счётчиком: свежий кадр важнее старого.
//!
//! **Порядок прихода команд и намерений объектива.** И команда (`Open`, `SwitchTo`), и намерение
//! экрана меняют цель камеры, и побеждать должно то, что пришло позже. Поэтому команды и
//! просьба «прочитай ящик намерений» получают номера из одного счётчика прихода, и поток
//! разбирает их по возрастанию номера. Полос по-прежнему три, очередь команд ограничена, а
//! просьба остаётся флагом (повторы склеиваются, переполнения нет). Склейка хранит номер
//! **последнего** толчка: движок читает самое свежее намерение, и оно новее любой команды,
//! пришедшей между толчками. Остаточное окно: слот намерений пишется раньше толчка, поэтому
//! намерение, записанное, но ещё не подтолкнувшее, может быть прочитано чуть раньше своего
//! номера; это всегда «новее», а не «старее», так что порядок «позже побеждает» не нарушается.
//!
//! Отправители (колбэки NDK на потоках binder, UI-поток) **никогда не ждут**: они берут замок
//! на микросекунды, кладут запись и будят поток. Остановка и просроченный таймер движка
//! тоже приходят через ящик: [`CameraInbox::wait_next`].

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use crate::backend::{BackendEvent, BackendEventListener};
use crate::command::{CameraCommand, CommandId, CommandRejected};
use crate::config::QueueConfig;

/// Что поток камеры получил из ящика.
#[derive(Debug, Clone, PartialEq)]
pub enum InboxItem {
    /// Просят остановиться.
    Stop,
    BackendEvent(BackendEvent),
    Command {
        id: CommandId,
        command: CameraCommand,
    },
    /// Кто-то снаружи попросил исполнителя экспозиции перечитать решение (см. [`ThreeAWaker`]).
    ThreeARefresh,
    /// В ящике намерений объектива появилось новое (см. [`LensIntentWaker`]).
    LensIntentRefresh,
    /// Наступил срок таймера движка, а другого содержимого нет.
    Timeout,
}

#[derive(Default)]
struct InboxState {
    rare_events: VecDeque<BackendEvent>,
    /// Команды в порядке прихода; первый элемент — с наименьшим номером прихода.
    commands: VecDeque<QueuedCommand>,
    capture_events: VecDeque<BackendEvent>,
    dropped_capture_events: u64,
    /// Сколько принятых, но не выполненных команд сброшено при остановке потока
    /// ([`CameraInbox::mark_stopped`]). Их отправители ответа не получат — это явный учёт.
    commands_discarded_at_stop: u64,
    next_command_id: u64,
    /// Общий счётчик прихода команд и толчков намерений объектива (см. заголовок модуля).
    next_arrival: u64,
    /// Есть непрочитанная просьба пересмотреть план экспозиции. Флаг, а не очередь: сколько бы
    /// раз ни разбудили, движок проснётся один раз (склейка пробуждений).
    three_a_refresh_requested: bool,
    /// Номер прихода последнего непрочитанного толчка «загляни в ящик намерений объектива».
    /// Не очередь: намерения «последнее побеждает», копить пробуждения незачем.
    lens_intent_refresh_arrival: Option<u64>,
    stop_requested: bool,
    /// Поток завершился: команды больше не принимаются, события отбрасываются.
    stopped: bool,
}

/// Принятая команда с номером прихода.
struct QueuedCommand {
    arrival: u64,
    id: CommandId,
    command: CameraCommand,
}

impl InboxState {
    /// Следующий номер прихода (общий для команд и толчков намерений).
    fn next_arrival_number(&mut self) -> u64 {
        self.next_arrival += 1;
        self.next_arrival
    }

    /// Толчок намерений стоит в очереди раньше первой команды (или команд нет): его читать первым.
    fn lens_refresh_is_due_before_commands(&self) -> bool {
        match (self.lens_intent_refresh_arrival, self.commands.front()) {
            (Some(_), None) => true,
            (Some(refresh), Some(command)) => refresh < command.arrival,
            (None, _) => false,
        }
    }
}

/// Ящик потока камеры.
pub struct CameraInbox {
    queues: QueueConfig,
    state: Mutex<InboxState>,
    wake: Condvar,
}

impl CameraInbox {
    pub fn new(queues: QueueConfig) -> Self {
        Self {
            queues,
            state: Mutex::new(InboxState::default()),
            wake: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, InboxState> {
        // Паника держателя замка не должна ронять колбэки NDK: данные ящика всегда целы.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Положить команду. Номер выдаётся здесь, по нему итог находится в событиях.
    pub fn submit_command(&self, command: CameraCommand) -> Result<CommandId, CommandRejected> {
        let mut state = self.lock();
        // Команда, принятая после `request_stop`, не получила бы ответа: поток уже не читает
        // команды. Поэтому отказ `EngineStopped` начинается с просьбы об остановке, а не с
        // её завершения (иначе между `request_stop` и `mark_stopped` — окно «принято без ответа»).
        if state.stop_requested || state.stopped {
            return Err(CommandRejected::EngineStopped);
        }
        if state.commands.len() >= self.queues.command_capacity {
            return Err(CommandRejected::QueueFull);
        }
        state.next_command_id += 1;
        let id = CommandId(state.next_command_id);
        let arrival = state.next_arrival_number();
        state.commands.push_back(QueuedCommand { arrival, id, command });
        drop(state);
        self.wake.notify_one();
        Ok(id)
    }

    /// Попросить поток остановиться (идемпотентно).
    pub fn request_stop(&self) {
        self.lock().stop_requested = true;
        self.wake.notify_one();
    }

    /// Пометить поток завершённым: после этого команды получают `EngineStopped`.
    ///
    /// Команды, которые успели лечь в очередь до `request_stop`, но поток их уже не разберёт,
    /// ответа не получают (поток остановлен, события наружу некому публиковать). Это решение
    /// не молчаливое: число таких команд пишется в журнал и доступно через
    /// [`CameraInbox::commands_discarded_at_stop`].
    pub fn mark_stopped(&self) {
        let mut state = self.lock();
        state.stopped = true;
        let discarded = state.commands.len() as u64;
        state.commands_discarded_at_stop += discarded;
        if discarded > 0 {
            tracing::warn!(
                target: "fasticamera::camera::inbox",
                discarded,
                "при остановке потока камеры сброшены принятые, но не выполненные команды"
            );
        }
        state.commands.clear();
        state.three_a_refresh_requested = false;
        state.lens_intent_refresh_arrival = None;
        state.rare_events.clear();
        state.capture_events.clear();
    }

    /// Сколько частых событий вытеснено за всё время.
    pub fn dropped_capture_events(&self) -> u64 {
        self.lock().dropped_capture_events
    }

    /// Сколько принятых команд сброшено без ответа при остановке потока.
    pub fn commands_discarded_at_stop(&self) -> u64 {
        self.lock().commands_discarded_at_stop
    }

    /// Сколько команд ждёт (для тестов и диагностики).
    pub fn pending_commands(&self) -> usize {
        self.lock().commands.len()
    }

    /// Попросить движок пересмотреть план экспозиции. Никогда не ждёт; повторы до чтения
    /// склеиваются в одно пробуждение. После остановки потока — no-op.
    fn request_three_a_refresh(&self) {
        let mut state = self.lock();
        if state.stopped {
            return;
        }
        state.three_a_refresh_requested = true;
        drop(state);
        self.wake.notify_one();
    }

    /// Попросить движок прочитать ящик намерений объектива. Никогда не ждёт; повторы
    /// склеиваются. После остановки потока — no-op.
    fn request_lens_intent_refresh(&self) {
        let mut state = self.lock();
        if state.stopped {
            return;
        }
        // Номер перезаписывается: берём последний толчок (см. заголовок модуля).
        state.lens_intent_refresh_arrival = Some(state.next_arrival_number());
        drop(state);
        self.wake.notify_one();
    }

    /// Подождать следующее содержимое, но не позже `deadline`. Вызывает только поток камеры.
    pub fn wait_next(&self, deadline: Option<Instant>) -> InboxItem {
        let mut state = self.lock();
        loop {
            if state.stop_requested {
                return InboxItem::Stop;
            }
            if let Some(event) = state.rare_events.pop_front() {
                return InboxItem::BackendEvent(event);
            }
            if std::mem::take(&mut state.three_a_refresh_requested) {
                return InboxItem::ThreeARefresh;
            }
            // Намерения и команды — по порядку прихода: позже пришедшее побеждает.
            if state.lens_refresh_is_due_before_commands() {
                state.lens_intent_refresh_arrival = None;
                return InboxItem::LensIntentRefresh;
            }
            if let Some(QueuedCommand { id, command, .. }) = state.commands.pop_front() {
                return InboxItem::Command { id, command };
            }
            // Срок таймера проверяется до частых событий: поток кадров не должен его заглушить.
            let now = Instant::now();
            if deadline.is_some_and(|due| now >= due) {
                return InboxItem::Timeout;
            }
            if let Some(event) = state.capture_events.pop_front() {
                return InboxItem::BackendEvent(event);
            }
            state = match deadline {
                Some(due) => {
                    self.wake
                        .wait_timeout(state, due.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self.wake.wait(state).unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

/// Ручка «пересмотри план экспозиции»: дёшево клонируется и безопасна для любого потока.
///
/// Нужна внешнему исполнителю экспозиции (наш AE в 08a живёт на потоке рендера и получает
/// решения не от камеры): он готовит решение и зовёт [`ThreeAWaker::wake`]; поток камеры
/// просыпается, спрашивает `ExposureController::refresh()` и, если план изменился, перестраивает
/// повторяющийся запрос. Пробуждение идёт мимо полосы кадров (её при перегрузке вытесняют) и
/// мимо очереди команд (она ограничена и может ответить `QueueFull`): это отдельный флаг, поэтому
/// просьба не теряется и не отнимает место у команд. В 08 снаружи её никто не зовёт — она нужна
/// 08a и тестам.
#[derive(Clone)]
pub struct ThreeAWaker {
    inbox: Arc<CameraInbox>,
}

impl ThreeAWaker {
    pub(crate) fn new(inbox: Arc<CameraInbox>) -> Self {
        Self { inbox }
    }

    /// Разбудить поток камеры для `refresh()`. Не блокирует; много вызовов подряд — одно пробуждение.
    pub fn wake(&self) {
        self.inbox.request_three_a_refresh();
    }
}

/// Толчок из ящика предложений AE (и самотолчок исполнителя) будит поток камеры так же, как
/// [`ThreeAWaker::wake`].
impl camera_contract::auto_exposure::RefreshSignal for ThreeAWaker {
    fn signal(&self) {
        self.wake();
    }
}

impl std::fmt::Debug for ThreeAWaker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ThreeAWaker")
    }
}

/// Толчок «в ящике намерений объектива новое»: будит поток камеры, и тот читает намерение.
/// Привязывается к ящику намерений при запуске [`crate::CameraRuntime`]. Не блокирует.
#[derive(Clone)]
pub struct LensIntentWaker {
    inbox: Arc<CameraInbox>,
}

impl LensIntentWaker {
    pub(crate) fn new(inbox: Arc<CameraInbox>) -> Self {
        Self { inbox }
    }
}

impl camera_contract::auto_exposure::RefreshSignal for LensIntentWaker {
    fn signal(&self) {
        self.inbox.request_lens_intent_refresh();
    }
}

impl std::fmt::Debug for LensIntentWaker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LensIntentWaker")
    }
}

impl BackendEventListener for CameraInbox {
    fn deliver(&self, event: BackendEvent) {
        let mut state = self.lock();
        if state.stopped {
            return;
        }
        if event.is_capture_stream_event() {
            if state.capture_events.len() >= self.queues.capture_event_capacity {
                state.capture_events.pop_front();
                state.dropped_capture_events += 1;
            }
            state.capture_events.push_back(event);
        } else {
            state.rare_events.push_back(event);
        }
        drop(state);
        self.wake.notify_one();
    }
}

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
