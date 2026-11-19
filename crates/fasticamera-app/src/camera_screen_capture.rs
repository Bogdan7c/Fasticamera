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
//! * отложенные ответы принадлежат ЭКРАНУ, которому они были обещаны: номера снимков свежего экрана начинаются
//!   с 1 заново, и ответ старого экрана «закрыл» бы чужой снимок с тем же номером (или занял бы место в очереди и
//!   дал ложный `Busy`). Поэтому при смене экрана (`attach`/`detach`) такие ответы снимаются со счётчиком;
//!   журнал запросов сохраняется (для отчётов сценария);
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
    /// Эпоха экрана: растёт при каждой установке и снятии. Ответ, обещанный в одной эпохе, в другой не доставляется.
    epoch: AtomicU64,
}

impl CaptureCompletionSlot {
    fn lock(&self) -> MutexGuard<'_, Option<ScreenInputInjector>> {
        self.target.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Новый экран установлен: ответы идут в его канал.
    pub fn attach(&self, injector: ScreenInputInjector) {
        *self.lock() = Some(injector);
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Экран снят: ответов адресовать некому.
    pub fn detach(&self) {
        *self.lock() = None;
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Текущая эпоха экрана: отложенный ответ запоминает её при постановке и сверяет при выдаче.
    fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
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
    /// Отложенные ответы, снятые при смене экрана (переустановка или снятие): они обещаны старому экрану.
    pub completions_dropped_on_screen_change: u64,
}

#[derive(Default)]
struct CounterCells {
    submitted: AtomicU64,
    delivered: AtomicU64,
    without_screen: AtomicU64,
    lost: AtomicU64,
    journal_evicted: AtomicU64,
    rejected_pending_full: AtomicU64,
    dropped_on_screen_change: AtomicU64,
}

/// Ответ, которому ещё не пришёл срок.
#[derive(Debug, Clone, Copy)]
struct PendingCompletion {
    /// С какого момента (часы кадров) можно ответить.
    due: MonotonicNanos,
    request: ShotRequest,
    /// Эпоха экрана на момент постановки (см. [`CaptureCompletionSlot`]).
    epoch: u64,
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
            completions_dropped_on_screen_change: self.counters.dropped_on_screen_change.load(Ordering::Relaxed),
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

    /// Снимает отложенные ответы, обещанные другому экрану (эпоха устарела); считает их, а не глотает молча.
    fn drop_stale_pending(&self, state: &mut FakeState) {
        let current_epoch = self.slot.current_epoch();
        let before = state.pending.len();
        state.pending.retain(|pending| pending.epoch == current_epoch);
        let dropped = (before - state.pending.len()) as u64;
        if dropped > 0 {
            self.counters
                .dropped_on_screen_change
                .fetch_add(dropped, Ordering::Relaxed);
            tracing::warn!(target: "fasticamera::camera_screen", dropped, "экран сменился: отложенные ответы фейк-снимка сняты");
        }
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
        // Каждый снимок виден в logcat: без этого «миниатюра появилась» на телефоне нельзя связать
        // с конкретным нажатием (затвор, громкость, таймер) — снимки редки, лог не шумит.
        tracing::info!(target: "fasticamera::camera_screen", ?request, "фейк-снимок принят");
        if self.delay.is_zero() {
            Self::record(&mut self.lock_state(), self.capacity, &self.counters, request);
            complete_saved(&self.slot, &self.counters, request);
            return SubmitOutcome::Accepted;
        }
        let mut state = self.lock_state();
        // Ответы старого экрана не должны ни занимать место в очереди, ни закрывать снимки нового.
        self.drop_stale_pending(&mut state);
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
            epoch: self.slot.current_epoch(),
        });
        SubmitOutcome::Accepted
    }

    fn poll_completions(&self, now: MonotonicNanos) {
        // Созревшие ответы достаём под замком, а отправляем уже без него (отправка берёт другой замок).
        let due: Vec<ShotRequest> = {
            let mut state = self.lock_state();
            self.drop_stale_pending(&mut state);
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
