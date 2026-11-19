//! Сервис снимка для экрана камеры (сессия 12): нейтральный контракт и ФЕЙК до настоящего сервиса (сессия 14).
//!
//! Экран только просит снимок ([`camera_screen::Effect::Capture`]) и ждёт `CaptureFinished` обратно. Кто и как
//! снимает — забота сервиса за трейтом [`CaptureService`]: сессия 14 подставит настоящий (RAW → ISP → файл),
//! экран и исполнитель эффектов при этом не изменятся.
//!
//! Фейк [`FakeCaptureService`]:
//! * записывает запросы в ОГРАНИЧЕННЫЙ журнал (ёмкость из конфига, вытеснение — со счётчиком);
//! * через `camera_screen.fake_capture_delay_ms` отвечает экрану `CaptureFinished(Saved)` через тот же канал входа,
//!   что касания (задержка 0 — ответ сразу);
//! * БЕЗ потоков: отложенные ответы лежат в очереди, которую источник UI опрашивает на каждом кадре
//!   ([`CaptureService::poll_completions`], время — `present_time` кадра). Поток на каждый снимок на потоке рендера
//!   был бы лишней аллокацией и неограниченным числом «сирот»;
//! * ответ идёт в ТЕКУЩИЙ экран: канал входа создаётся заново при каждой установке экрана, поэтому адрес ответа
//!   лежит в общей ячейке [`CaptureCompletionSlot`], которую сервис экрана переставляет при установке.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camera_screen::ScreenAction;
use camera_screen::shutter::{CaptureOutcome, ShotRequest};
use shell_contract::clock::MonotonicNanos;

use crate::camera_screen_input::{InputDelivery, ScreenInputInjector};

/// Почему сервис не принял запрос снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitRejection {
    /// Предыдущий снимок ещё обрабатывается.
    Busy,
    /// Сервис недоступен (не поднялся, остановлен).
    ServiceUnavailable,
}

/// Итог постановки запроса снимка. Итог СНИМКА придёт позже действием `CaptureFinished`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Запрос принят; ответ придёт `CaptureFinished`.
    Accepted,
    /// Запрос отвергнут; `CaptureFinished` НЕ придёт — исполнитель сам сообщит экрану о провале.
    Rejected(SubmitRejection),
}

/// Сервис снимка. Не блокирует: ставит запрос и возвращается.
pub trait CaptureService: Send + Sync {
    /// Ставит запрос. `now` — время показа текущего кадра UI (часы потока рендера).
    fn submit(&self, request: ShotRequest, now: MonotonicNanos) -> SubmitOutcome;

    /// Кадровый тик: сервис выдаёт ответы, чей срок к `now` настал. Вызывает источник UI в начале КАЖДОГО кадра.
    /// По умолчанию ничего (настоящий сервис отвечает своим путём).
    fn poll_completions(&self, _now: MonotonicNanos) {}
}

/// Куда фейк отвечает: канал входа текущего экрана (`None` — экрана сейчас нет).
#[derive(Default)]
pub struct CaptureCompletionSlot {
    target: Mutex<Option<ScreenInputInjector>>,
}

impl CaptureCompletionSlot {
    fn lock(&self) -> MutexGuard<'_, Option<ScreenInputInjector>> {
        self.target.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Новый экран установлен: ответы идут в его канал.
    pub fn attach(&self, injector: ScreenInputInjector) {
        *self.lock() = Some(injector);
    }

    /// Экран снят: ответов адресовать некому.
    pub fn detach(&self) {
        *self.lock() = None;
    }

    fn deliver(&self, action: ScreenAction) -> CompletionDelivery {
        // Адрес копируем из-под замка и отправляем уже без него.
        let target = self.lock().clone();
        match target {
            None => CompletionDelivery::NoScreen,
            Some(injector) => match injector.inject(action) {
                InputDelivery::Queued => CompletionDelivery::Delivered,
                InputDelivery::Overflowed | InputDelivery::ReceiverGone => CompletionDelivery::Lost,
            },
        }
    }
}

/// Куда делся ответ фейка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionDelivery {
    Delivered,
    /// Экрана в этот момент нет.
    NoScreen,
    /// Канал переполнен или экран уничтожен.
    Lost,
}

/// Счётчики фейка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FakeCaptureCounters {
    /// Запросы, принятые журналом.
    pub submitted: u64,
    /// Ответы `CaptureFinished`, дошедшие до канала экрана.
    pub completions_delivered: u64,
    /// Ответы, которым не нашлось экрана (снят на время сценария).
    pub completions_without_screen: u64,
    /// Ответы, потерянные каналом (переполнение или уничтоженный экран).
    pub completions_lost: u64,
    /// Старые записи журнала, вытесненные новыми (журнал ограничен конфигом).
    pub journal_evicted: u64,
    /// Запросы, отвергнутые как `Busy`: очередь отложенных ответов уже полна.
    pub rejected_pending_full: u64,
}

#[derive(Default)]
struct CounterCells {
    submitted: AtomicU64,
    delivered: AtomicU64,
    without_screen: AtomicU64,
    lost: AtomicU64,
    journal_evicted: AtomicU64,
    rejected_pending_full: AtomicU64,
}

/// Ответ, которому ещё не пришёл срок.
#[derive(Debug, Clone, Copy)]
struct PendingCompletion {
    /// С какого момента (часы кадров) можно ответить.
    due: MonotonicNanos,
    request: ShotRequest,
}

/// Состояние фейка под одним замком: журнал и очередь отложенных ответов. Обе структуры ограничены.
#[derive(Default)]
struct FakeState {
    journal: VecDeque<ShotRequest>,
    pending: VecDeque<PendingCompletion>,
}

/// Фейк-сервис снимка: ограниченный журнал запросов и отложенный ответ `Saved`.
pub struct FakeCaptureService {
    delay: Duration,
    /// Ёмкость журнала И очереди отложенных ответов (из конфига, не меньше 1).
    capacity: usize,
    slot: Arc<CaptureCompletionSlot>,
    state: Mutex<FakeState>,
    counters: CounterCells,
}

impl FakeCaptureService {
    /// Фейк с задержкой ответа, ёмкостью журнала и ячейкой адреса (её переставляет сервис экрана).
    pub fn new(delay: Duration, journal_capacity: usize, slot: Arc<CaptureCompletionSlot>) -> Self {
        Self {
            delay,
            // Конфиг уже запретил 0; `max(1)` — страховка от нулевой ёмкости, при которой журнал не вёл бы себя как журнал.
            capacity: journal_capacity.max(1),
            slot,
            state: Mutex::default(),
            counters: CounterCells::default(),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Последние принятые запросы по порядку (не больше ёмкости; более старые вытеснены и посчитаны).
    pub fn journal(&self) -> Vec<ShotRequest> {
        self.lock_state().journal.iter().copied().collect()
    }

    pub fn counters(&self) -> FakeCaptureCounters {
        FakeCaptureCounters {
            submitted: self.counters.submitted.load(Ordering::Relaxed),
            completions_delivered: self.counters.delivered.load(Ordering::Relaxed),
            completions_without_screen: self.counters.without_screen.load(Ordering::Relaxed),
            completions_lost: self.counters.lost.load(Ordering::Relaxed),
            journal_evicted: self.counters.journal_evicted.load(Ordering::Relaxed),
            rejected_pending_full: self.counters.rejected_pending_full.load(Ordering::Relaxed),
        }
    }

    /// Записывает запрос в журнал, вытесняя самый старый при переполнении.
    fn record(state: &mut FakeState, capacity: usize, counters: &CounterCells, request: ShotRequest) {
        if state.journal.len() >= capacity {
            state.journal.pop_front();
            counters.journal_evicted.fetch_add(1, Ordering::Relaxed);
        }
        state.journal.push_back(request);
        counters.submitted.fetch_add(1, Ordering::Relaxed);
    }

    /// Срок ответа: `now` + задержка. Насыщающее сложение: огромная задержка не должна переполнять `i64`.
    fn due_after(&self, now: MonotonicNanos) -> MonotonicNanos {
        let delay_nanos = i64::try_from(self.delay.as_nanos()).unwrap_or(i64::MAX);
        MonotonicNanos::from_nanos(now.as_nanos().saturating_add(delay_nanos))
    }
}

/// Отправляет ответ и записывает его судьбу в счётчики.
fn complete_saved(slot: &CaptureCompletionSlot, counters: &CounterCells, request: ShotRequest) {
    let action = ScreenAction::CaptureFinished {
        request: request.request,
        outcome: CaptureOutcome::Saved,
    };
    let cell = match slot.deliver(action) {
        CompletionDelivery::Delivered => &counters.delivered,
        CompletionDelivery::NoScreen => &counters.without_screen,
        CompletionDelivery::Lost => &counters.lost,
    };
    cell.fetch_add(1, Ordering::Relaxed);
}

impl CaptureService for FakeCaptureService {
    fn submit(&self, request: ShotRequest, now: MonotonicNanos) -> SubmitOutcome {
        if self.delay.is_zero() {
            Self::record(&mut self.lock_state(), self.capacity, &self.counters, request);
            complete_saved(&self.slot, &self.counters, request);
            return SubmitOutcome::Accepted;
        }
        let mut state = self.lock_state();
        // Копить отложенные ответы без предела нельзя: полная очередь — честное «занято».
        if state.pending.len() >= self.capacity {
            self.counters.rejected_pending_full.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(target: "fasticamera::camera_screen", "очередь ответов фейк-снимка полна: запрос отвергнут как занятый");
            return SubmitOutcome::Rejected(SubmitRejection::Busy);
        }
        Self::record(&mut state, self.capacity, &self.counters, request);
        state.pending.push_back(PendingCompletion {
            due: self.due_after(now),
            request,
        });
        SubmitOutcome::Accepted
    }

    fn poll_completions(&self, now: MonotonicNanos) {
        // Созревшие ответы достаём под замком, а отправляем уже без него (отправка берёт другой замок).
        let due: Vec<ShotRequest> = {
            let mut state = self.lock_state();
            let mut due = Vec::new();
            while state.pending.front().is_some_and(|pending| pending.due <= now) {
                if let Some(pending) = state.pending.pop_front() {
                    due.push(pending.request);
                }
            }
            due
        };
        for request in due {
            complete_saved(&self.slot, &self.counters, request);
        }
    }
}

#[cfg(test)]
#[path = "camera_screen_capture_tests.rs"]
mod tests;
