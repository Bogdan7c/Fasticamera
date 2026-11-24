//! Тесты сохранения кэша пайплайнов: подставное хранилище (в том числе медленное, с «воротами»), настоящий контекст
//! `Headless` (RADV), вспомогательный поток сохранения.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::pipeline_cache::{
    DEFAULT_PIPELINE_CACHE_MAX_BYTES, PipelineCacheOrigin, seal_cache_file, unseal_cache_file,
};
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::validation::ValidationMode;

use super::*;

const PATIENCE: Duration = Duration::from_secs(20);

/// Хранилище в памяти: что записали, сколько раз, и можно ли сломать запись или чтение или задержать запись.
#[derive(Debug, Default)]
struct MemoryStore {
    data: Mutex<Option<Vec<u8>>>,
    saves: AtomicUsize,
    writers_now: AtomicUsize,
    max_writers_at_once: AtomicUsize,
    fail_writes: bool,
    fail_reads: bool,
    gate: Option<Mutex<GateEnds>>,
}

/// Ворота медленной записи: запись сообщает, что вошла, и ждёт сигнала.
#[derive(Debug)]
struct GateEnds {
    entered: Sender<()>,
    release: Receiver<()>,
}

struct GateControl {
    entered: Receiver<()>,
    release: Sender<()>,
}

fn gated_store() -> (MemoryStore, GateControl) {
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel();
    let store = MemoryStore {
        gate: Some(Mutex::new(GateEnds {
            entered: entered_tx,
            release: release_rx,
        })),
        ..MemoryStore::default()
    };
    (
        store,
        GateControl {
            entered: entered_rx,
            release: release_tx,
        },
    )
}

impl PipelineCacheStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, PipelineCacheStoreError> {
        if self.fail_reads {
            return Err(PipelineCacheStoreError::Read {
                place: "память".to_owned(),
                reason: "тестовый отказ чтения".to_owned(),
            });
        }
        Ok(self.data.lock().unwrap().clone())
    }

    fn save(&self, data: &[u8]) -> Result<(), PipelineCacheStoreError> {
        let now = self.writers_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_writers_at_once.fetch_max(now, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            let gate = gate.lock().unwrap();
            let _ = gate.entered.send(());
            // Ошибка приёма (ворота брошены) — просто идём дальше: тест закончился.
            let _ = gate.release.recv_timeout(PATIENCE);
        }
        let result = if self.fail_writes {
            Err(PipelineCacheStoreError::Write {
                place: "память".to_owned(),
                reason: "тестовый отказ записи".to_owned(),
            })
        } else {
            *self.data.lock().unwrap() = Some(data.to_vec());
            self.saves.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        self.writers_now.fetch_sub(1, Ordering::SeqCst);
        result
    }
}

fn context_or_skip_with(file: Vec<u8>) -> Option<GpuContext> {
    let mut config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    config.pipeline_cache_data = file;
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error}");
                None
            } else {
                panic!("GPU недоступен: {error}");
            }
        }
    }
}

fn context_or_skip() -> Option<GpuContext> {
    context_or_skip_with(Vec::new())
}

/// Создаёт пайплайн: кэш контекста от этого растёт.
fn grow_cache(context: &GpuContext) -> ComputePipeline {
    ComputePipeline::new(
        context,
        &ComputePipelineDesc {
            label: "saver-test",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap()
}

fn shared(store: MemoryStore) -> (Arc<MemoryStore>, SharedPipelineCacheStore) {
    let store = Arc::new(store);
    let shared: SharedPipelineCacheStore = store.clone();
    (store, shared)
}

fn saver_for(context: &GpuContext, store: SharedPipelineCacheStore, interval: Duration) -> PipelineCacheSaver {
    PipelineCacheSaver::new(context.clone(), Some(store), interval, DEFAULT_PIPELINE_CACHE_MAX_BYTES)
}

#[test]
fn a_grown_cache_is_saved_on_the_worker_with_timings_and_not_again_while_nothing_changed() {
    let Some(context) = context_or_skip() else { return };
    let (store, shared_store) = shared(MemoryStore::default());
    let mut saver = saver_for(&context, shared_store, Duration::ZERO);
    let _pipeline = grow_cache(&context);
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    assert!(saver.wait_until_idle(PATIENCE));
    let stats = saver.stats();
    let written = store.data.lock().unwrap().clone().expect("кэш записан в хранилище");
    assert_eq!(stats.last_outcome, Some(SaveOutcome::Saved { bytes: written.len() }));
    assert!(unseal_cache_file(&written).is_ok(), "записан запечатанный файл");
    assert!(
        stats.last_fetch > Duration::ZERO,
        "время получения данных измерено: {stats:?}"
    );
    assert_eq!(stats.saved, 1);
    // Ничего не выросло: второй проход ничего не пишет.
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    assert!(saver.wait_until_idle(PATIENCE));
    assert_eq!(saver.stats().last_outcome, Some(SaveOutcome::Unchanged));
    assert_eq!(store.saves.load(Ordering::SeqCst), 1);
}

#[test]
fn the_render_thread_tick_never_waits_for_a_slow_store_and_overlapping_requests_never_overlap_writes() {
    let Some(context) = context_or_skip() else { return };
    let (store, control) = gated_store();
    let (store, shared_store) = shared(store);
    let mut saver = saver_for(&context, shared_store, Duration::ZERO);
    let _pipeline = grow_cache(&context);
    let started = Instant::now();
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    control
        .entered
        .recv_timeout(PATIENCE)
        .expect("запись началась и застряла в воротах");
    // Запись застряла: тики потока рендера возвращаются сразу и не плодят вторую запись.
    for _ in 0..5 {
        assert_eq!(saver.tick(Instant::now()), TickOutcome::AlreadyPending);
    }
    assert!(started.elapsed() < PATIENCE / 2, "тик не ждал медленное хранилище");
    assert_eq!(store.writers_now.load(Ordering::SeqCst), 1);
    control.release.send(()).unwrap();
    assert!(saver.wait_until_idle(PATIENCE));
    let stats = saver.stats();
    assert_eq!((stats.requests, stats.coalesced, stats.saved), (1, 5, 1));
    assert_eq!(
        store.max_writers_at_once.load(Ordering::SeqCst),
        1,
        "две записи одновременно не шли"
    );
}

#[test]
fn dropping_the_saver_in_the_middle_of_a_write_waits_for_it_and_makes_the_last_save() {
    let Some(context) = context_or_skip() else { return };
    let (store, control) = gated_store();
    let (store, shared_store) = shared(store);
    let mut saver = saver_for(&context, shared_store, Duration::ZERO);
    let _first = grow_cache(&context);
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    control.entered.recv_timeout(PATIENCE).unwrap();
    // Пока запись застряла, кэш вырос ещё: последнее сохранение при Drop обязано его захватить.
    let _second = grow_cache(&context);
    let _third = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "saver-test-2",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 2,
        },
    )
    .unwrap();
    let release = control.release.clone();
    let opener = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        // Две записи: текущая и последняя при остановке.
        release.send(()).unwrap();
        release.send(()).unwrap();
    });
    drop(saver);
    opener.join().unwrap();
    assert!(
        store.saves.load(Ordering::SeqCst) >= 1,
        "запись дождалась ворот и завершилась"
    );
    assert_eq!(store.max_writers_at_once.load(Ordering::SeqCst), 1);
    let last = store.data.lock().unwrap().clone().unwrap();
    assert_eq!(
        last.len(),
        context.pipeline_cache_data().unwrap().len(),
        "последнее сохранение захватило рост, случившийся во время записи"
    );
}

#[test]
fn a_foreign_driver_block_is_replaced_by_the_first_growth_and_the_next_context_really_uses_it() {
    // Файл корректен для НАШЕЙ обёртки, но внутри блок другого устройства (после обновления ОС).
    let Some(donor) = context_or_skip() else { return };
    let _pipeline = grow_cache(&donor);
    let real = donor.pipeline_cache_data().unwrap();
    let mut foreign_payload = unseal_cache_file(&real).unwrap().to_vec();
    for byte in &mut foreign_payload[16..32] {
        *byte ^= 0xFF;
    }
    let foreign_file = seal_cache_file(&foreign_payload);
    let (store, shared_store) = shared(MemoryStore {
        data: Mutex::new(Some(foreign_file.clone())),
        ..MemoryStore::default()
    });

    let Ok((context, mut saver)) = PipelineCacheSaver::create_context(
        GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL),
        &PipelineCacheConfig {
            store: Some(shared_store.clone()),
            save_interval: Duration::ZERO,
            max_file_bytes: DEFAULT_PIPELINE_CACHE_MAX_BYTES,
        },
    ) else {
        return;
    };
    eprintln!("исход при чужом блоке: {:?}", context.pipeline_cache_origin());
    // База сравнения — фактическое состояние кэша, а не размер чужого блока.
    let baseline = context.pipeline_cache_initial_file_bytes();
    let _pipeline = grow_cache(&context);
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    assert!(saver.wait_until_idle(PATIENCE));
    assert!(
        matches!(saver.stats().last_outcome, Some(SaveOutcome::Saved { bytes }) if bytes > baseline),
        "рост относительно фактического состояния заменил негодный файл: {:?}",
        saver.stats()
    );
    let replaced = store.data.lock().unwrap().clone().unwrap();
    assert_ne!(replaced, foreign_file);
    drop(saver);

    // Следующий запуск берёт уже наш файл, и драйвер его реально принимает.
    let next = PipelineCacheSaver::create_context(
        GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL),
        &PipelineCacheConfig {
            store: Some(shared_store),
            save_interval: Duration::from_secs(3600),
            max_file_bytes: DEFAULT_PIPELINE_CACHE_MAX_BYTES,
        },
    )
    .unwrap();
    assert!(matches!(
        next.0.pipeline_cache_origin(),
        PipelineCacheOrigin::Loaded { .. }
    ));
    assert!(next.0.pipeline_cache_initial_file_bytes() > baseline);
}

#[test]
fn an_oversized_cache_is_not_saved_and_an_oversized_file_starts_empty() {
    let Some(context) = context_or_skip() else { return };
    let (store, shared_store) = shared(MemoryStore::default());
    let mut saver = PipelineCacheSaver::new(context.clone(), Some(shared_store), Duration::ZERO, 16);
    let _pipeline = grow_cache(&context);
    assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
    assert!(saver.wait_until_idle(PATIENCE));
    assert!(matches!(saver.stats().last_outcome, Some(SaveOutcome::TooLarge { .. })));
    assert_eq!(saver.stats().too_large, 1);
    assert!(store.data.lock().unwrap().is_none(), "больше предела — файл не пишется");

    // Старый файл больше предела: запуск с пустым кэшем, а следующее сохранение его заменяет.
    let big = seal_cache_file(&vec![7_u8; 4096]);
    let (store, shared_store) = shared(MemoryStore {
        data: Mutex::new(Some(big.clone())),
        ..MemoryStore::default()
    });
    let Ok((context, mut saver)) = PipelineCacheSaver::create_context(
        GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL),
        &PipelineCacheConfig {
            store: Some(shared_store),
            save_interval: Duration::ZERO,
            max_file_bytes: big.len() - 1,
        },
    ) else {
        return;
    };
    assert_eq!(
        context.pipeline_cache_origin(),
        PipelineCacheOrigin::OversizedFile { bytes: big.len() }
    );
    let _pipeline = grow_cache(&context);
    // Выросший кэш (≈ 1–2 КБ) меньше предела: заменяет большой файл.
    if context.pipeline_cache_data().unwrap().len() < big.len() {
        assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
        assert!(saver.wait_until_idle(PATIENCE));
        assert_ne!(store.data.lock().unwrap().clone().unwrap(), big);
    }
}

#[test]
fn failed_writes_are_counted_apart_and_retried_and_a_missing_store_is_named() {
    let Some(context) = context_or_skip() else { return };
    let (store, shared_store) = shared(MemoryStore {
        fail_writes: true,
        ..MemoryStore::default()
    });
    let mut saver = saver_for(&context, shared_store, Duration::ZERO);
    let _pipeline = grow_cache(&context);
    for expected_failures in 1..=2 {
        assert_eq!(saver.tick(Instant::now()), TickOutcome::Requested);
        assert!(saver.wait_until_idle(PATIENCE));
        assert_eq!(
            saver.stats().write_failed,
            expected_failures,
            "размер не считается сохранённым"
        );
    }
    assert_eq!(saver.stats().last_outcome, Some(SaveOutcome::WriteFailed));
    assert!(store.data.lock().unwrap().is_none());

    let mut without_store = PipelineCacheSaver::new(context, None, Duration::ZERO, DEFAULT_PIPELINE_CACHE_MAX_BYTES);
    assert_eq!(without_store.tick(Instant::now()), TickOutcome::NoStore);
}

#[test]
fn the_tick_waits_for_its_interval_between_requests() {
    let Some(context) = context_or_skip() else { return };
    let (_, shared_store) = shared(MemoryStore::default());
    let interval = Duration::from_secs(10);
    let mut saver = saver_for(&context, shared_store, interval);
    let start = Instant::now();
    assert_eq!(saver.tick(start), TickOutcome::NotDue);
    assert_eq!(saver.tick(start + interval), TickOutcome::Requested);
    assert!(saver.wait_until_idle(PATIENCE));
    assert_eq!(
        saver.tick(start + interval + Duration::from_secs(1)),
        TickOutcome::NotDue
    );
}

#[test]
fn initial_data_comes_from_the_store_and_read_failures_fall_back_to_an_empty_cache() {
    let (_, with_data) = shared(MemoryStore {
        data: Mutex::new(Some(vec![1, 2, 3])),
        ..MemoryStore::default()
    });
    assert_eq!(initial_pipeline_cache_data(Some(&with_data)), vec![1, 2, 3]);
    let (_, empty) = shared(MemoryStore::default());
    assert!(
        initial_pipeline_cache_data(Some(&empty)).is_empty(),
        "первый запуск — файла нет"
    );
    let (_, broken) = shared(MemoryStore {
        fail_reads: true,
        ..MemoryStore::default()
    });
    assert!(
        initial_pipeline_cache_data(Some(&broken)).is_empty(),
        "ошибка чтения не мешает запуску"
    );
    assert!(initial_pipeline_cache_data(None).is_empty());
}
