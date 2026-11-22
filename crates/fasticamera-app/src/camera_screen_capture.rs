//! Сервис снимка для экрана камеры (сессия 12, две фазы — сессия 14): нейтральный контракт и ФЕЙК до настоящего
//! сервиса.
//!
//! Экран только просит снимок ([`camera_screen::Effect::Capture`], с полным [`ShotRequest`], включая момент
//! нажатия) и ждёт два ответа обратно (решение 140): `CaptureSecured` («кадр захвачен», затвор свободен) и
//! `CaptureFinished` («сохранён» или «ошибка»). Кто и как снимает — забота сервиса за трейтом [`CaptureService`]:
//! настоящий (кольцо кадров → ISP → файл) подставляется без изменений экрана и исполнителя эффектов. Трейт для
//! двух фаз НЕ менялся: обе фазы приходят тем же путём — действиями в канал входа экрана.
//!
//! Фейк [`FakeCaptureService`]:
//! * записывает запросы в ОГРАНИЧЕННЫЙ журнал (ёмкость из конфига, вытеснение — со счётчиком);
//! * через `camera_screen.fake_capture_secured_delay_ms` отвечает `CaptureSecured`, а через
//!   `camera_screen.fake_capture_delay_ms` — `CaptureFinished(Saved)`, через тот же канал входа, что касания
//!   (задержка 0 — ответ сразу). Захват не может случиться позже сохранения: срок `Secured` не больше срока `Saved`;
//! * БЕЗ потоков: отложенные ответы лежат в очереди, которую источник UI опрашивает на каждом кадре
//!   ([`CaptureService::poll_completions`], время — `present_time` кадра). Поток на каждый снимок на потоке рендера
//!   был бы лишней аллокацией и неограниченным числом «сирот»;
//! * отложенные ответы принадлежат ЭКРАНУ, которому они были обещаны: номера снимков свежего экрана начинаются
//!   с 1 заново, и ответ старого экрана «закрыл» бы чужой снимок с тем же номером (или занял бы место в очереди и
//!   дал ложный `Busy`). Поэтому при смене экрана (`attach`/`detach`) такие ответы снимаются со счётчиком;
//!   журнал запросов сохраняется (для отчётов сценария);
//! * фейк отвечает БЕЗ картинки миниатюры (14g): `CaptureFinished(Saved)` обычным действием, и экран рисует серый
//!   диск «снимок есть». Это осознанно: у фейка нет пикселей, а сценарии с фейком проверяют экран, не миниатюру;
//! * ответ идёт в ТЕКУЩИЙ экран: канал входа создаётся заново при каждой установке экрана, поэтому адрес ответа
//!   лежит в общей ячейке [`CaptureCompletionSlot`], которую сервис экрана переставляет при установке.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camera_screen::ScreenAction;
use camera_screen::shutter::{CaptureOutcome, ShotRequest, ShotRequestId};
use shell_contract::clock::MonotonicNanos;
use ui_draw::picture::UiPicture;

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
    ///
    /// Эпоха растёт ПОД замком адреса: иначе ответ старой эпохи, проверенный между сменой адреса и ростом
    /// эпохи, попал бы в канал нового экрана (номера запросов свежего экрана начинаются с 1 заново).
    pub fn attach(&self, injector: ScreenInputInjector) {
        let mut target = self.lock();
        *target = Some(injector);
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Экран снят: ответов адресовать некому.
    pub fn detach(&self) {
        let mut target = self.lock();
        *target = None;
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    /// Текущая эпоха экрана: ответ запоминает её при постановке запроса и сверяет при выдаче.
    pub(crate) fn current_epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }

    fn deliver(&self, action: ScreenAction) -> CompletionDelivery {
        // Адрес копируем из-под замка и отправляем уже без него.
        let target = self.lock().clone();
        Self::inject(target, action)
    }

    /// Доставка ответа, обещанного в эпохе `epoch`: чужая эпоха (экран сменился) — ответ НЕ доставляется,
    /// иначе он закрыл бы снимок нового экрана с тем же номером. Сверка и копия адреса — под одним замком.
    pub(crate) fn deliver_in_epoch(&self, epoch: u64, action: ScreenAction) -> EpochDelivery {
        self.deliver_message_in_epoch(epoch, |injector| injector.inject(action))
    }

    /// Доставка итога «сохранён» с картинкой (14g) под тем же правилом эпохи: сообщение одно, поэтому картинка
    /// и итог доходят до экрана вместе или не доходят вообще.
    pub(crate) fn deliver_saved_with_picture_in_epoch(
        &self,
        epoch: u64,
        request: ShotRequestId,
        picture: UiPicture,
    ) -> EpochDelivery {
        self.deliver_message_in_epoch(epoch, |injector| injector.inject_saved_with_picture(request, picture))
    }

    /// Общая часть доставки в эпоху: сверка эпохи и копия адреса — под одним замком, отправка — без него.
    fn deliver_message_in_epoch(
        &self,
        epoch: u64,
        send: impl FnOnce(&ScreenInputInjector) -> InputDelivery,
    ) -> EpochDelivery {
        let target = {
            let target = self.lock();
            if self.current_epoch() != epoch {
                return EpochDelivery::StaleScreen;
            }
            target.clone()
        };
        match target {
            None => EpochDelivery::NoScreen,
            Some(injector) => match send(&injector) {
                InputDelivery::Queued => EpochDelivery::Delivered,
                InputDelivery::Overflowed | InputDelivery::ReceiverGone => EpochDelivery::Lost,
            },
        }
    }

    fn inject(target: Option<ScreenInputInjector>, action: ScreenAction) -> CompletionDelivery {
        match target {
            None => CompletionDelivery::NoScreen,
            Some(injector) => match injector.inject(action) {
                InputDelivery::Queued => CompletionDelivery::Delivered,
                InputDelivery::Overflowed | InputDelivery::ReceiverGone => CompletionDelivery::Lost,
            },
        }
    }
}

/// Куда делся ответ настоящей службы снимков (она сверяет эпоху экрана, фейк — нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpochDelivery {
    Delivered,
    /// Экран сменился после постановки запроса: ответ принадлежал старому экрану и снят.
    StaleScreen,
    /// Экрана в этот момент нет.
    NoScreen,
    /// Канал переполнен или экран уничтожен.
    Lost,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct FakeCaptureCounters {
    /// Запросы, принятые журналом.
    pub submitted: u64,
    /// Ответы `CaptureSecured` («кадр захвачен»), дошедшие до канала экрана.
    pub secured_delivered: u64,
    /// Итоги `CaptureFinished(Saved)`, дошедшие до канала экрана (по одному на снимок).
    pub completions_delivered: u64,
    /// Ответы любой фазы, которым не нашлось экрана (снят на время сценария).
    pub completions_without_screen: u64,
    /// Ответы любой фазы, потерянные каналом (переполнение или уничтоженный экран).
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
    secured_delivered: AtomicU64,
    delivered: AtomicU64,
    without_screen: AtomicU64,
    lost: AtomicU64,
    journal_evicted: AtomicU64,
    rejected_pending_full: AtomicU64,
    dropped_on_screen_change: AtomicU64,
}

/// Фаза ответа фейка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionPhase {
    /// «Кадр захвачен» (`CaptureSecured`).
    Secured,
    /// «Сохранён» (`CaptureFinished(Saved)`).
    Saved,
}

/// Ответ, которому ещё не пришёл срок.
#[derive(Debug, Clone, Copy)]
struct PendingCompletion {
    /// С какого момента (часы кадров) можно ответить.
    due: MonotonicNanos,
    request: ShotRequestId,
    phase: CompletionPhase,
    /// Эпоха экрана на момент постановки (см. [`CaptureCompletionSlot`]).
    epoch: u64,
}

/// Сроки ответов фейка: через сколько после запроса приходят две фазы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeCaptureTiming {
    /// До `CaptureSecured`. Не больше `saved_after`: захват не бывает позже сохранения.
    pub secured_after: Duration,
    /// До `CaptureFinished(Saved)`.
    pub saved_after: Duration,
}

impl FakeCaptureTiming {
    /// Срок захвата, зажатый сроком сохранения.
    fn effective_secured_after(&self) -> Duration {
        self.secured_after.min(self.saved_after)
    }
}

/// Состояние фейка под одним замком: журнал и очередь отложенных ответов. Обе структуры ограничены.
#[derive(Default)]
struct FakeState {
    journal: VecDeque<ShotRequest>,
    pending: VecDeque<PendingCompletion>,
}

/// Фейк-сервис снимка: ограниченный журнал запросов и отложенный ответ `Saved`.
pub struct FakeCaptureService {
    timing: FakeCaptureTiming,
    /// Ёмкость журнала И очереди отложенных ответов (из конфига, не меньше 1).
    capacity: usize,
    slot: Arc<CaptureCompletionSlot>,
    state: Mutex<FakeState>,
    counters: CounterCells,
}

impl FakeCaptureService {
    /// Фейк со сроками ответов, ёмкостью журнала и ячейкой адреса (её переставляет сервис экрана).
    pub fn new(timing: FakeCaptureTiming, journal_capacity: usize, slot: Arc<CaptureCompletionSlot>) -> Self {
        Self {
            timing,
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
        self.lock_state().journal.iter().cloned().collect()
    }

    pub fn counters(&self) -> FakeCaptureCounters {
        FakeCaptureCounters {
            submitted: self.counters.submitted.load(Ordering::Relaxed),
            secured_delivered: self.counters.secured_delivered.load(Ordering::Relaxed),
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
    fn due_after(now: MonotonicNanos, delay: Duration) -> MonotonicNanos {
        let delay_nanos = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
        MonotonicNanos::from_nanos(now.as_nanos().saturating_add(delay_nanos))
    }

    /// Сколько снимков ещё ждут итога сохранения (запись `Saved` в очереди — по одной на снимок).
    fn outstanding_shots(state: &FakeState) -> usize {
        state
            .pending
            .iter()
            .filter(|pending| pending.phase == CompletionPhase::Saved)
            .count()
    }
}

/// Отправляет ответ нужной фазы и записывает его судьбу в счётчики.
fn complete(slot: &CaptureCompletionSlot, counters: &CounterCells, request: ShotRequestId, phase: CompletionPhase) {
    let (action, delivered) = match phase {
        CompletionPhase::Secured => (ScreenAction::CaptureSecured { request }, &counters.secured_delivered),
        CompletionPhase::Saved => (
            ScreenAction::CaptureFinished {
                request,
                outcome: CaptureOutcome::Saved,
            },
            &counters.delivered,
        ),
    };
    let cell = match slot.deliver(action) {
        CompletionDelivery::Delivered => delivered,
        CompletionDelivery::NoScreen => &counters.without_screen,
        CompletionDelivery::Lost => &counters.lost,
    };
    cell.fetch_add(1, Ordering::Relaxed);
}

impl CaptureService for FakeCaptureService {
    fn submit(&self, request: ShotRequest, now: MonotonicNanos) -> SubmitOutcome {
        // Каждый снимок виден в logcat: без этого «миниатюра появилась» на телефоне нельзя связать
        // с конкретным нажатием (затвор, громкость, таймер) — снимки редки, лог не шумит.
        tracing::info!(target: "fasticamera::camera_screen", request = ?request.request, pressed_at = ?request.pressed_at, "фейк-снимок принят");
        let id = request.request;
        let secured_after = self.timing.effective_secured_after();
        let mut state = self.lock_state();
        // Ответы старого экрана не должны ни занимать место в очереди, ни закрывать снимки нового.
        self.drop_stale_pending(&mut state);
        // Копить отложенные ответы без предела нельзя: полная очередь — честное «занято».
        if Self::outstanding_shots(&state) >= self.capacity {
            self.counters.rejected_pending_full.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(target: "fasticamera::camera_screen", "очередь ответов фейк-снимка полна: запрос отвергнут как занятый");
            return SubmitOutcome::Rejected(SubmitRejection::Busy);
        }
        Self::record(&mut state, self.capacity, &self.counters, request);
        let epoch = self.slot.current_epoch();
        // Фазы без задержки отвечают сразу (после выхода из-под замка: отправка берёт другой замок), остальные
        // ждут срока кадровых часов.
        let mut immediate = Vec::new();
        for (phase, delay) in [
            (CompletionPhase::Secured, secured_after),
            (CompletionPhase::Saved, self.timing.saved_after),
        ] {
            if delay.is_zero() {
                immediate.push(phase);
            } else {
                state.pending.push_back(PendingCompletion {
                    due: Self::due_after(now, delay),
                    request: id,
                    phase,
                    epoch,
                });
            }
        }
        drop(state);
        for phase in immediate {
            complete(&self.slot, &self.counters, id, phase);
        }
        SubmitOutcome::Accepted
    }

    fn poll_completions(&self, now: MonotonicNanos) {
        // Созревшие ответы достаём под замком, а отправляем уже без него (отправка берёт другой замок).
        let mut due: Vec<PendingCompletion> = {
            let mut state = self.lock_state();
            self.drop_stale_pending(&mut state);
            let (ready, waiting): (VecDeque<_>, VecDeque<_>) =
                state.pending.drain(..).partition(|pending| pending.due <= now);
            state.pending = waiting;
            ready.into_iter().collect()
        };
        // По сроку, а при равных — в порядке постановки: `Secured` снимка всегда раньше его `Saved`.
        due.sort_by_key(|pending| pending.due);
        for pending in due {
            complete(&self.slot, &self.counters, pending.request, pending.phase);
        }
    }
}

#[cfg(test)]
#[path = "camera_screen_capture_tests.rs"]
mod tests;
