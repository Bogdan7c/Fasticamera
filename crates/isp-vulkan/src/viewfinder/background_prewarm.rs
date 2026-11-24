//! Фоновая заготовка вариантов пайплайна ([`BackgroundPrewarm`]): рабочий поток компилирует варианты семейства,
//! пока поток рендера рисует кадры.
//!
//! Зачем: компиляция одного варианта на Adreno 740 — 145–180 мс, семейство из 12 вариантов — 1,5–1,8 с. На потоке
//! рендера это остановка первого кадра при каждом запуске. Здесь поток рендера ждёт только вариант первого кадра, а
//! остальное делает фон; готовое подхватывается между кадрами без ожидания.
//!
//! # Кто чем владеет
//! * Модуль владеет очередью заданий, списком готовых пайплайнов и рабочим потоком. Всё состояние — под ОДНИМ замком
//!   ([`QueueState`]); замок никогда не держится на время компиляции.
//! * Кэш вариантов ([`super::variants::VariantCache`]) решает, ЧТО заготавливать и куда класть готовое. Модуль
//!   отвечает на три вопроса: «поставь в очередь» ([`BackgroundPrewarm::enqueue`]), «отдай готовое»
//!   ([`BackgroundPrewarm::take_ready`]) и «мне прямо сейчас нужен этот ключ» ([`BackgroundPrewarm::claim`]).
//!
//! # Политика «кадру нужен вариант, а фон его ещё не выдал» ([`Claim`])
//! | Где ключ | Что делает кадр | Почему |
//! |---|---|---|
//! | уже готов | берёт готовый | бесплатно |
//! | в очереди, ещё не начат | вынимает его из очереди и компилирует сам (+ одна компиляция в кадре) | фон не будет делать то же второй раз |
//! | компилируется фоном прямо сейчас | ждёт именно это задание (не дольше лимита) | ждать не дольше, чем компилировать самому, и без двойной работы |
//! | фон не успел за лимит или умер | компилирует сам; поздний результат фона отбрасывается и считается | кадр не ждёт вечно |
//!
//! # Безопасность потоков
//! Пайплайн варианта создаётся целиком в одном вызове `ComputePipeline::new`: свой layout набора дескрипторов, свой
//! пул, свой layout пайплайна, свой шейдерный модуль; общего `VkPipelineCache` нет (передаётся `VK_NULL_HANDLE`).
//! Спецификация Vulkan (Threading Behavior) требует внешней синхронизации только объектов, переданных как
//! изменяемые параметры; `device` для создания таких объектов — нет. Счётчики живых объектов — атомарные.
//! Готовый пайплайн ещё НИ РАЗУ не записывался в командный буфер, поэтому его можно уничтожить на любом потоке без
//! ожидания GPU.
//!
//! # Завершение
//! `Drop` ставит флаг остановки, чистит очередь и дожидается рабочего потока: он закончит текущую компиляцию (её
//! нельзя отменить — не дольше одного варианта) и выйдет. Неподобранные готовые пайплайны уничтожаются вместе с
//! общим состоянием. Паника задания ловится в потоке: она не роняет рендер, а превращается в отказ заготовки
//! ([`ReadyBatch::aborted`]), дальше варианты создаются по требованию.

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpu_vulkan::compute::ComputePipeline;

use super::error::ViewfinderError;
use super::variants::VariantKey;

/// Как создать пайплайн варианта (настоящий создатель кэша или подставной в тестах). Вызывается на рабочем потоке.
pub(crate) type BuildJob = Arc<dyn Fn(VariantKey) -> Result<ComputePipeline, ViewfinderError> + Send + Sync>;

/// Итог одного фонового задания.
pub(crate) struct PrewarmResult {
    /// Какой вариант собирали.
    pub key: VariantKey,
    /// Готовый пайплайн или причина отказа.
    pub outcome: Result<ComputePipeline, ViewfinderError>,
    /// Сколько длилась компиляция (CPU-время рабочего потока).
    pub compile: Duration,
}

/// Забранные готовые результаты и сигнал отказа заготовки.
pub(crate) struct ReadyBatch {
    /// Готовое с прошлого раза.
    pub results: Vec<PrewarmResult>,
    /// `Some(причина)` — рабочий поток прервался (паника задания): новых результатов не будет. Выдаётся один раз.
    pub aborted: Option<String>,
}

/// Ответ на «этому кадру прямо сейчас нужен ключ».
pub(crate) enum Claim {
    /// Фон про этот ключ не знает (не ставился в очередь или уже забран).
    Unknown,
    /// Ключ был в очереди и не начат: он вынут из очереди, кадр компилирует его сам.
    Withdrawn,
    /// Результат фона (готов сразу или дождались текущей компиляции).
    Delivered(Box<PrewarmResult>),
    /// Фон компилировал ключ, но не уложился в лимит ожидания (или умер): кадр компилирует сам, а поздний результат
    /// фона отбросит кэш.
    TimedOut,
}

/// Всё общее состояние под одним замком.
#[derive(Default)]
struct QueueState {
    /// Ждут своей очереди (в порядке срочности: первым берут голову).
    queued: VecDeque<VariantKey>,
    /// Ключ, который рабочий поток компилирует прямо сейчас.
    in_flight: Option<VariantKey>,
    /// Готовое, ещё не подобранное потоком рендера.
    ready: Vec<PrewarmResult>,
    /// Попросили остановиться (`Drop`).
    stop: bool,
    /// Причина аварийной остановки (паника задания); `None` — всё штатно.
    abort: Option<String>,
    /// Причина уже выдана потоку рендера.
    abort_reported: bool,
    /// Когда очередь опустела после последнего задания (`None` — работа есть или заданий не было).
    drained_at: Option<Instant>,
}

/// Общая часть: состояние, условная переменная и способ создания.
struct Shared {
    state: Mutex<QueueState>,
    changed: Condvar,
    build: BuildJob,
    created_at: Instant,
}

impl Shared {
    /// Замок состояния. Отравление игнорируется осознанно: под замком только простые операции над коллекциями, а
    /// паника компиляции происходит вне замка и ловится.
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Берёт следующее задание. `block` — ждать появления (рабочий поток) или вернуть `None` сразу (ручной привод).
    /// `None` и при остановке, и при аварии: работать больше нечего.
    fn take_job(&self, block: bool) -> Option<VariantKey> {
        let mut state = self.lock();
        loop {
            if state.stop || state.abort.is_some() {
                return None;
            }
            if let Some(key) = state.queued.pop_front() {
                state.in_flight = Some(key);
                state.drained_at = None;
                return Some(key);
            }
            if !block {
                return None;
            }
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Выполняет взятое задание вне замка и публикует итог. `false` — задание упало паникой, работать дальше нельзя.
    fn execute(&self, key: VariantKey) -> bool {
        let started = Instant::now();
        let built = catch_unwind(AssertUnwindSafe(|| (self.build)(key)));
        let compile = started.elapsed();
        let mut discarded = None;
        let healthy = {
            let mut state = self.lock();
            state.in_flight = None;
            let healthy = match built {
                Ok(outcome) if !state.stop => {
                    state.ready.push(PrewarmResult { key, outcome, compile });
                    true
                }
                // Остановка во время компиляции: результат никому не нужен, уничтожится ниже, вне замка.
                Ok(outcome) => {
                    discarded = Some(outcome);
                    true
                }
                Err(payload) => {
                    state.abort = Some(panic_message(payload.as_ref()));
                    state.queued.clear();
                    false
                }
            };
            if state.queued.is_empty() {
                state.drained_at = Some(Instant::now());
            }
            healthy
        };
        self.changed.notify_all();
        drop(discarded);
        healthy
    }

    /// Рабочий цикл потока.
    fn work_loop(&self) {
        while let Some(key) = self.take_job(true) {
            if !self.execute(key) {
                break;
            }
        }
    }
}

/// Текст паники для журнала.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "паника без текста".to_owned()
    }
}

/// Фоновая заготовка: очередь заданий и рабочий поток (или ручной привод в тестах).
pub(crate) struct BackgroundPrewarm {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

/// Как запустить рабочий поток: шов, в тестах подставляется отказ ОС.
pub(crate) type SpawnWorker = dyn FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>>;

impl BackgroundPrewarm {
    /// Запускает рабочий поток. Ошибка — ОС не дала поток: вызывающий выбирает запасной путь.
    pub fn spawn(build: BuildJob) -> std::io::Result<Self> {
        Self::spawn_with(
            build,
            Box::new(|work| {
                std::thread::Builder::new()
                    .name("isp-variant-prewarm".into())
                    .spawn(work)
            }),
        )
    }

    /// То же с заданным способом запуска потока (шов для тестов отказа ОС).
    pub fn spawn_with(build: BuildJob, spawner: Box<SpawnWorker>) -> std::io::Result<Self> {
        let shared = Shared::new(build);
        let for_worker = Arc::clone(&shared);
        let worker = spawner(Box::new(move || for_worker.work_loop()))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// Ставит в очередь ключи, которых там ещё нет (ни в очереди, ни в работе, ни среди готовых). Порядок — порядок
    /// срочности. Возвращает, сколько действительно добавлено. После остановки и аварии — 0.
    pub fn enqueue(&self, keys: impl IntoIterator<Item = VariantKey>) -> usize {
        let mut state = self.shared.lock();
        if state.stop || state.abort.is_some() {
            return 0;
        }
        let mut added = 0;
        for key in keys {
            let known = state.queued.contains(&key)
                || state.in_flight == Some(key)
                || state.ready.iter().any(|result| result.key == key);
            if !known {
                state.queued.push_back(key);
                added += 1;
            }
        }
        if added > 0 {
            state.drained_at = None;
            drop(state);
            self.shared.changed.notify_all();
        }
        added
    }

    /// Сколько заданий ещё не выполнено (в очереди и в работе).
    #[cfg(test)]
    pub fn unfinished(&self) -> usize {
        let state = self.shared.lock();
        state.queued.len() + usize::from(state.in_flight.is_some())
    }

    /// Сколько мест кэша уже «обещано»: невыполненные задания и готовые, ещё не подобранные.
    pub fn reserved_slots(&self) -> usize {
        let state = self.shared.lock();
        state.queued.len() + usize::from(state.in_flight.is_some()) + state.ready.len()
    }

    /// Забирает готовое без ожидания. Причина аварии выдаётся один раз.
    pub fn take_ready(&self) -> ReadyBatch {
        let mut state = self.shared.lock();
        let results = std::mem::take(&mut state.ready);
        let aborted = if state.abort_reported {
            None
        } else {
            state.abort_reported = state.abort.is_some();
            state.abort.clone()
        };
        ReadyBatch { results, aborted }
    }

    /// Кадру прямо сейчас нужен `key`: см. таблицу политики в описании модуля. Ждёт не дольше `wait_limit` и только
    /// если именно этот ключ компилируется.
    pub fn claim(&self, key: &VariantKey, wait_limit: Duration) -> Claim {
        let deadline = Instant::now() + wait_limit;
        let mut state = self.shared.lock();
        if let Some(position) = state.queued.iter().position(|queued| queued == key) {
            state.queued.remove(position);
            return Claim::Withdrawn;
        }
        loop {
            if let Some(position) = state.ready.iter().position(|result| result.key == *key) {
                return Claim::Delivered(Box::new(state.ready.remove(position)));
            }
            if state.in_flight != Some(*key) {
                return Claim::Unknown;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Claim::TimedOut;
            };
            if state.abort.is_some() {
                return Claim::TimedOut;
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Ждёт, пока все задания выполнены (или фон прервался); `true` — очередь пуста и ничего не в работе.
    /// Для тестов и диагностики: поток рендера этим не пользуется.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if state.queued.is_empty() && state.in_flight.is_none() {
                return true;
            }
            if state.abort.is_some() {
                return false;
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .shared
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Через сколько после создания фон выполнил последнее задание (`None` — работа ещё идёт или заданий не было).
    pub fn drained_after(&self) -> Option<Duration> {
        let state = self.shared.lock();
        state
            .drained_at
            .map(|drained_at| drained_at.saturating_duration_since(self.shared.created_at))
    }
}

impl Shared {
    fn new(build: BuildJob) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState::default()),
            changed: Condvar::new(),
            build,
            created_at: Instant::now(),
        })
    }
}

impl Drop for BackgroundPrewarm {
    /// Останавливает поток детерминированно: не дольше одной текущей компиляции.
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.stop = true;
            state.queued.clear();
        }
        self.shared.changed.notify_all();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!(
                target: "fasticamera::isp_viewfinder",
                "рабочий поток заготовки вариантов завершился паникой вне задания"
            );
        }
    }
}

/// Ручной привод фона для тестов: задания выполняются на вызывающем потоке по команде теста, без гонок.
#[cfg(any(test, feature = "test-support"))]
pub struct ManualPrewarmDriver {
    shared: Arc<Shared>,
}

#[cfg(any(test, feature = "test-support"))]
impl ManualPrewarmDriver {
    /// Выполняет одно задание из головы очереди; `false` — очередь пуста (или заготовка остановлена).
    pub fn run_next(&self) -> bool {
        match self.shared.take_job(false) {
            Some(key) => {
                self.shared.execute(key);
                true
            }
            None => false,
        }
    }

    /// Выполняет всё, что в очереди; возвращает число выполненных заданий.
    pub fn run_all(&self) -> usize {
        let mut done = 0;
        while self.run_next() {
            done += 1;
        }
        done
    }

    /// Сколько заданий ждёт в очереди.
    pub fn queued(&self) -> usize {
        self.shared.lock().queued.len()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl BackgroundPrewarm {
    /// Фон без потока: задания двигает [`ManualPrewarmDriver`].
    pub fn manual(build: BuildJob) -> (Self, ManualPrewarmDriver) {
        let shared = Shared::new(build);
        let driver = ManualPrewarmDriver {
            shared: Arc::clone(&shared),
        };
        (Self { shared, worker: None }, driver)
    }
}

#[cfg(test)]
#[path = "background_prewarm_tests.rs"]
mod tests;
