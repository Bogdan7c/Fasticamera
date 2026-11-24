//! Хранилище кэша пайплайнов между запусками приложения ([`PipelineCacheStore`]) и его сохранение вспомогательным
//! потоком ([`PipelineCacheSaver`]).
//!
//! Зачем: драйвер Adreno 740 между запусками ничего не помнит, и каждый запуск тратит 150–200 мс на каждый вариант
//! шейдера. Сохранённый кэш (`VkPipelineCache`) сокращает создание варианта до долей миллисекунды (замер I6: 162 мс →
//! 0,7 мс на первый вариант, весь запуск до первого живого кадра 1,4 с → 0,7 с).
//!
//! Границы:
//! * как и где лежат байты (файл в папке приложения) — забота реализации [`PipelineCacheStore`] (в приложении); она
//!   содержимого не знает и кладёт/отдаёт байты как есть;
//! * формат файла (магическое число, версия, длина, контрольная сумма), проверка при загрузке, предел размера и
//!   исходы загрузки — у `gpu_vulkan::pipeline_cache`; битый, чужой или слишком большой файл до драйвера не доходит;
//! * когда сохранять и как не мешать кадрам — здесь: поток рендера на тике ТОЛЬКО решает «пора» (без обращения к
//!   драйверу и к диску), а получение данных, сравнение и запись идут на вспомогательном потоке.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::error::GpuInitError;

/// Название потока сохранения.
const SAVER_THREAD_NAME: &str = "pipeline-cache-saver";

/// Почему хранилище не прочитало или не записало кэш. Это не провал запуска: без кэша приложение работает, только
/// стартует медленнее.
#[derive(Debug, thiserror::Error)]
pub enum PipelineCacheStoreError {
    /// Чтение не удалось (файл есть, но не читается).
    #[error("чтение кэша пайплайнов {place}: {reason}")]
    Read {
        /// Где (путь файла для журнала).
        place: String,
        /// Текст ошибки ОС.
        reason: String,
    },
    /// Запись не удалась (диск полон, нет прав).
    #[error("запись кэша пайплайнов {place}: {reason}")]
    Write {
        /// Где (путь файла для журнала).
        place: String,
        /// Текст ошибки ОС.
        reason: String,
    },
}

/// Куда кладут и откуда берут байты файла кэша пайплайнов.
pub trait PipelineCacheStore: std::fmt::Debug + Send + Sync {
    /// Сохранённые байты; `Ok(None)` — их ещё нет (первый запуск), это не ошибка.
    fn load(&self) -> Result<Option<Vec<u8>>, PipelineCacheStoreError>;
    /// Заменяет сохранённые байты целиком. Реализация обязана не оставлять полузаписанный файл (убитый процесс не
    /// должен подсунуть следующему запуску обрезок) и сбросить данные на диск до подмены файла.
    fn save(&self, data: &[u8]) -> Result<(), PipelineCacheStoreError>;
}

/// Подключённое хранилище (общее для настройки бэкенда и сохранителя).
pub type SharedPipelineCacheStore = Arc<dyn PipelineCacheStore>;

/// Байты для создания контекста: что хранилище отдало (пусто — чистый кэш). Ошибка чтения не мешает запуску.
pub fn initial_pipeline_cache_data(store: Option<&SharedPipelineCacheStore>) -> Vec<u8> {
    let Some(store) = store else {
        return Vec::new();
    };
    match store.load() {
        Ok(Some(data)) => data,
        Ok(None) => Vec::new(),
        Err(error) => {
            tracing::warn!(target: "fasticamera::render::vulkan", %error, "кэш пайплайнов не прочитан: запуск с пустым");
            Vec::new()
        }
    }
}

/// Настройки кэша пайплайнов в конфиге бэкенда.
#[derive(Debug, Clone)]
pub struct PipelineCacheConfig {
    /// Где хранится кэш между запусками (`None` — не хранится: ПК-стенд и тесты).
    pub store: Option<SharedPipelineCacheStore>,
    /// Как часто поток рендера решает «пора проверить, не вырос ли кэш». К 10 с все варианты ISP давно созданы (фон
    /// кончает за 2–3 с); проверка и запись идут на вспомогательном потоке и кадров не касаются.
    pub save_interval: Duration,
    /// Наибольший файл кэша, байт: больше — запуск с пустым кэшем, а больший кэш не сохраняется. Кэш только растёт
    /// (каждая сборка с изменёнными шейдерами добавляет записи, старые не уходят), предел не даёт файлу пухнуть без конца.
    pub max_file_bytes: usize,
}

impl Default for PipelineCacheConfig {
    fn default() -> Self {
        Self {
            store: None,
            save_interval: Duration::from_secs(10),
            max_file_bytes: gpu_vulkan::pipeline_cache::DEFAULT_PIPELINE_CACHE_MAX_BYTES,
        }
    }
}

/// Что решил тик потока рендера (само сохранение идёт на вспомогательном потоке).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// Хранилища нет: сохранять некуда.
    NoStore,
    /// Вспомогательный поток не запустился (ОС не дала): сохранение отключено, причина в журнале.
    NoWorker,
    /// Ещё не время (интервал не истёк).
    NotDue,
    /// Поручено вспомогательному потоку.
    Requested,
    /// Прошлое поручение ещё выполняется или ждёт: второе не создаётся, записи не накладываются.
    AlreadyPending,
}

/// Чем кончилось одно сохранение на вспомогательном потоке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Сохранено столько байт.
    Saved {
        /// Размер записанного файла.
        bytes: usize,
    },
    /// Кэш не вырос относительно того, что уже на диске (или было загружено): писать нечего.
    Unchanged,
    /// Файл получился больше предела: не пишется (следующий запуск всё равно взял бы пустой кэш).
    TooLarge {
        /// Размер файла.
        bytes: usize,
    },
    /// Данные у драйвера получить не удалось.
    FetchFailed,
    /// Хранилище не записало файл.
    WriteFailed,
}

/// Счётчики и времена сохранителя (накопительные, кроме `last_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SaverStats {
    /// Сколько поручений принято от потока рендера.
    pub requests: u64,
    /// Сколько «пора» слились с уже ожидающим поручением (записи не накладываются).
    pub coalesced: u64,
    /// Сохранений файла.
    pub saved: u64,
    /// Проходов без записи: кэш не вырос.
    pub unchanged: u64,
    /// Проходов, где файл превысил предел.
    pub too_large: u64,
    /// Отказов получения данных у драйвера.
    pub fetch_failed: u64,
    /// Отказов записи хранилищем.
    pub write_failed: u64,
    /// Длительность последнего `vkGetPipelineCacheData`.
    pub last_fetch: Duration,
    /// Длительность последней записи файла (только если запись была).
    pub last_write: Duration,
    /// Исход последнего прохода.
    pub last_outcome: Option<SaveOutcome>,
}

#[derive(Default)]
struct SaverState {
    /// Поток рендера поручил проход.
    requested: bool,
    /// Вспомогательный поток выполняет проход прямо сейчас.
    busy: bool,
    /// `Drop`: сделать последнее сохранение и выйти.
    stop: bool,
    stats: SaverStats,
}

struct SaverShared {
    state: Mutex<SaverState>,
    changed: Condvar,
}

impl SaverShared {
    fn lock(&self) -> MutexGuard<'_, SaverState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Всё, что нужно вспомогательному потоку для прохода сохранения (владеет им поток).
struct SavePass {
    context: GpuContext,
    store: SharedPipelineCacheStore,
    max_file_bytes: usize,
    /// Размер файла на диске (или фактический размер кэша сразу после создания): расти нужно относительно него.
    baseline_file_bytes: usize,
}

impl SavePass {
    /// Один проход: данные → сравнение → запись. Возвращает исход и времена.
    fn run(&mut self) -> (SaveOutcome, Duration, Option<Duration>) {
        let fetch_started = Instant::now();
        let data = self.context.pipeline_cache_data();
        let fetch = fetch_started.elapsed();
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                tracing::warn!(target: "fasticamera::render::vulkan", %error, "данные кэша пайплайнов не получены");
                return (SaveOutcome::FetchFailed, fetch, None);
            }
        };
        if data.len() > self.max_file_bytes {
            tracing::warn!(
                target: "fasticamera::render::vulkan",
                bytes = data.len(),
                limit = self.max_file_bytes,
                "кэш пайплайнов больше предела: не сохраняется"
            );
            return (SaveOutcome::TooLarge { bytes: data.len() }, fetch, None);
        }
        // Рост относительно ФАКТИЧЕСКОГО состояния, а не размера когда-то предложенного блока: драйвер мог молча
        // проигнорировать чужой блок, и тогда любой рост от пустого кэша заменяет негодный файл.
        if data.len() <= self.baseline_file_bytes {
            return (SaveOutcome::Unchanged, fetch, None);
        }
        let write_started = Instant::now();
        let written = self.store.save(&data);
        let write = write_started.elapsed();
        match written {
            Ok(()) => {
                tracing::info!(
                    target: "fasticamera::render::vulkan",
                    bytes = data.len(),
                    fetch_ms = fetch.as_secs_f64() * 1e3,
                    write_ms = write.as_secs_f64() * 1e3,
                    "кэш пайплайнов сохранён"
                );
                self.baseline_file_bytes = data.len();
                (SaveOutcome::Saved { bytes: data.len() }, fetch, Some(write))
            }
            Err(error) => {
                tracing::warn!(target: "fasticamera::render::vulkan", %error, "кэш пайплайнов не сохранён");
                (SaveOutcome::WriteFailed, fetch, Some(write))
            }
        }
    }
}

/// Рабочий цикл вспомогательного потока: ждёт поручения, выполняет проход; при остановке делает последний и выходит.
fn saver_loop(shared: &SaverShared, mut pass: SavePass) {
    loop {
        let stopping = {
            let mut state = shared.lock();
            while !state.requested && !state.stop {
                state = shared.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            state.requested = false;
            state.busy = true;
            state.stop
        };
        // Замок не держится на время обращения к драйверу и диску: тик потока рендера не блокируется.
        let (outcome, fetch, write) = pass.run();
        {
            let mut state = shared.lock();
            state.busy = false;
            let stats = &mut state.stats;
            stats.last_fetch = fetch;
            if let Some(write) = write {
                stats.last_write = write;
            }
            stats.last_outcome = Some(outcome);
            match outcome {
                SaveOutcome::Saved { .. } => stats.saved += 1,
                SaveOutcome::Unchanged => stats.unchanged += 1,
                SaveOutcome::TooLarge { .. } => stats.too_large += 1,
                SaveOutcome::FetchFailed => stats.fetch_failed += 1,
                SaveOutcome::WriteFailed => stats.write_failed += 1,
            }
        }
        shared.changed.notify_all();
        if stopping {
            return;
        }
    }
}

/// Сохраняет кэш пайплайнов контекста в хранилище: раз в `interval`, если он вырос, и при уничтожении. Тяжёлое
/// (данные драйвера, диск) идёт на вспомогательном потоке.
pub struct PipelineCacheSaver {
    shared: Arc<SaverShared>,
    worker: Option<JoinHandle<()>>,
    has_store: bool,
    interval: Duration,
    last_check: Instant,
}

impl PipelineCacheSaver {
    /// Создаёт контекст GPU с кэшем пайплайнов из хранилища и сохранитель при нём. Нет хранилища или файла — чистый
    /// кэш; битый, чужой и слишком большой файл до драйвера не доходит, запуск от этого не ломается.
    pub fn create_context(
        mut context_config: GpuContextConfig,
        config: &PipelineCacheConfig,
    ) -> Result<(GpuContext, Self), GpuInitError> {
        context_config.pipeline_cache_data = initial_pipeline_cache_data(config.store.as_ref());
        context_config.pipeline_cache_max_bytes = config.max_file_bytes;
        let context = GpuContext::new(&context_config)?;
        tracing::info!(
            target: "fasticamera::render::vulkan",
            origin = ?context.pipeline_cache_origin(),
            offered_bytes = context_config.pipeline_cache_data.len(),
            actual_bytes = context.pipeline_cache_initial_file_bytes(),
            "кэш пайплайнов подготовлен"
        );
        let saver = Self::new(
            context.clone(),
            config.store.clone(),
            config.save_interval,
            config.max_file_bytes,
        );
        Ok((context, saver))
    }

    /// Сохранитель для готового контекста. База сравнения — фактический размер кэша сразу после создания.
    pub fn new(
        context: GpuContext,
        store: Option<SharedPipelineCacheStore>,
        interval: Duration,
        max_file_bytes: usize,
    ) -> Self {
        let shared = Arc::new(SaverShared {
            state: Mutex::new(SaverState::default()),
            changed: Condvar::new(),
        });
        let has_store = store.is_some();
        let mut worker = None;
        if let Some(store) = store {
            let pass = SavePass {
                baseline_file_bytes: context.pipeline_cache_initial_file_bytes(),
                context,
                store,
                max_file_bytes,
            };
            let for_worker = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name(SAVER_THREAD_NAME.to_owned())
                .spawn(move || saver_loop(&for_worker, pass))
            {
                Ok(handle) => worker = Some(handle),
                Err(error) => {
                    tracing::error!(
                        target: "fasticamera::render::vulkan",
                        %error,
                        "ОС не дала поток сохранения кэша пайплайнов: кэш не будет сохраняться"
                    );
                }
            }
        }
        Self {
            shared,
            worker,
            has_store,
            interval,
            last_check: Instant::now(),
        }
    }

    /// Вызывается потоком рендера между кадрами. Решает только «пора ли»: к драйверу и диску не обращается.
    pub fn tick(&mut self, now: Instant) -> TickOutcome {
        if !self.has_store {
            return TickOutcome::NoStore;
        }
        if self.worker.is_none() {
            return TickOutcome::NoWorker;
        }
        if now.saturating_duration_since(self.last_check) < self.interval {
            return TickOutcome::NotDue;
        }
        self.last_check = now;
        let mut state = self.shared.lock();
        if state.requested || state.busy {
            state.stats.coalesced += 1;
            return TickOutcome::AlreadyPending;
        }
        state.requested = true;
        state.stats.requests += 1;
        drop(state);
        self.shared.changed.notify_all();
        TickOutcome::Requested
    }

    /// Счётчики и времена сохранителя.
    pub fn stats(&self) -> SaverStats {
        self.shared.lock().stats
    }

    /// Ждёт, пока вспомогательный поток выполнит поручение (или истечёт `timeout`); `true` — он свободен. Для тестов и
    /// диагностики: поток рендера этим не пользуется.
    pub fn wait_until_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        while state.requested || state.busy {
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
        true
    }
}

impl Drop for PipelineCacheSaver {
    /// Остановка бэкенда: вспомогательный поток доделывает текущую запись, делает последнее сохранение выросшего кэша и
    /// выходит; здесь мы его дожидаемся.
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.stop = true;
        }
        self.shared.changed.notify_all();
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!(target: "fasticamera::render::vulkan", "поток сохранения кэша пайплайнов завершился паникой");
        }
    }
}

#[cfg(test)]
#[path = "pipeline_cache_store_tests.rs"]
mod tests;
