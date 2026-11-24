//! Тесты фоновой заготовки на настоящем GPU (`Headless`, validation).
//!
//! Детерминизм: фон двигается руками ([`ManualPrewarmDriver`]) либо настоящим потоком с «воротами» — подставным
//! созданием, которое тест держит закрытым ровно столько, сколько нужно. Проверяется результат: сколько вариантов где
//! создано, что лежит в кэше, что видно в счётчиках и что не осталось живых объектов.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::validation::ValidationMode;

use super::*;
use crate::tone_base::ToneBaseKind;
use crate::viewfinder::awb_group_layout::AwbGroupLayout;
use crate::viewfinder::awb_reduction::AwbReduction;
use crate::viewfinder::config::VariantPrewarm;
use crate::viewfinder::retirement::RecordSerial;
use crate::viewfinder::variant_family::{VARIANTS_PER_STAGE_SET, product_key};
use crate::viewfinder::variants::VariantCache;

/// Сколько ждём в тестах там, где ожидание не должно истекать.
const PATIENCE: Duration = Duration::from_secs(20);

fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var("FASTICAMERA_SKIP_GPU_TESTS").as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error}");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите FASTICAMERA_SKIP_GPU_TESTS=1, чтобы пропустить осознанно");
            }
        }
    }
}

fn new_cache(capacity: usize) -> VariantCache {
    VariantCache::new(
        capacity,
        LocalSize::new(8, 8).unwrap(),
        2,
        AwbReduction::SharedMemoryWindow,
    )
}

fn key(tone_base: ToneBaseKind) -> VariantKey {
    product_key(tone_base, AwbGroupLayout::General)
}

/// Перед созданием вызывается «крючок»: тест держит ворота, подставляет отказ или панику. Затем — настоящее создание.
type Hook = Arc<dyn Fn(&VariantKey) -> Result<(), ViewfinderError> + Send + Sync>;

fn job_with(cache: &VariantCache, context: &GpuContext, hook: Hook) -> BuildJob {
    let real = cache.background_build_job(context);
    Arc::new(move |variant| {
        hook(&variant)?;
        real(variant)
    })
}

fn plain_job(cache: &VariantCache, context: &GpuContext) -> BuildJob {
    job_with(cache, context, Arc::new(|_| Ok(())))
}

/// Ворота настоящего фонового потока: «вошёл в создание» и «выпусти».
struct Gate {
    entered: Receiver<VariantKey>,
    release: Sender<()>,
}

fn gated_job(cache: &VariantCache, context: &GpuContext) -> (BuildJob, Gate) {
    let (entered_tx, entered_rx) = channel();
    let (release_tx, release_rx) = channel::<()>();
    let entered_tx = Mutex::new(entered_tx);
    let release_rx = Mutex::new(release_rx);
    let hook: Hook = Arc::new(move |variant| {
        entered_tx.lock().unwrap().send(*variant).unwrap();
        // Ошибка приёма (ворота брошены) — просто идём дальше: тест кончился.
        let _ = release_rx.lock().unwrap().recv_timeout(PATIENCE);
        Ok(())
    });
    (
        job_with(cache, context, hook),
        Gate {
            entered: entered_rx,
            release: release_tx,
        },
    )
}

fn prepare(cache: &mut VariantCache, context: &GpuContext, variant: VariantKey, serial: u64) {
    let evicted = cache
        .prepare(context, variant, RecordSerial(serial), VariantPrewarm::InBackground)
        .unwrap();
    assert!(evicted.is_none(), "в этих тестах кэш не тесный");
}

// ---------- очередь ----------

#[test]
fn queue_builds_in_order_dedupes_and_leaves_no_live_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let cache = new_cache(48);
        let (background, driver) = BackgroundPrewarm::manual(plain_job(&cache, &context));
        let (first, second, third) = (
            key(ToneBaseKind::Legacy),
            key(ToneBaseKind::Agx),
            key(ToneBaseKind::Filmic),
        );
        assert_eq!(background.enqueue([first, second, third]), 3);
        assert_eq!(background.enqueue([second, third]), 0, "уже в очереди: дублей нет");
        assert_eq!(background.unfinished(), 3);
        assert!(driver.run_next());
        assert_eq!(driver.queued(), 2);
        assert_eq!(driver.run_all(), 2);
        assert!(!driver.run_next(), "очередь пуста");
        assert_eq!(
            background.enqueue([first]),
            0,
            "готовое, но не подобранное, тоже известно"
        );
        let batch = background.take_ready();
        let order: Vec<_> = batch.results.iter().map(|result| result.key).collect();
        assert_eq!(order, vec![first, second, third], "порядок срочности сохранён");
        assert!(batch.results.iter().all(|result| result.outcome.is_ok()));
        assert!(batch.aborted.is_none());
        assert_eq!(background.unfinished(), 0);
        assert!(background.drained_after().is_some(), "время окончания зафиксировано");
        assert!(background.take_ready().results.is_empty(), "забрано один раз");
    }
    assert_eq!(
        context.live_objects(),
        baseline,
        "готовые и неподобранные пайплайны уничтожены"
    );
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn claim_distinguishes_unknown_withdrawn_and_delivered() {
    let Some(context) = gpu_context_or_skip() else { return };
    let cache = new_cache(48);
    let (background, driver) = BackgroundPrewarm::manual(plain_job(&cache, &context));
    let (queued, finished) = (key(ToneBaseKind::Agx), key(ToneBaseKind::Aces));
    background.enqueue([finished, queued]);
    assert!(driver.run_next());
    assert!(matches!(
        background.claim(&key(ToneBaseKind::Filmic), Duration::ZERO),
        Claim::Unknown
    ));
    assert!(matches!(background.claim(&queued, Duration::ZERO), Claim::Withdrawn));
    assert_eq!(driver.queued(), 0, "вынутый из очереди ключ фон делать не будет");
    match background.claim(&finished, Duration::ZERO) {
        Claim::Delivered(result) => assert!(result.outcome.is_ok() && result.key == finished),
        _ => panic!("готовый ключ обязан быть выдан"),
    }
    assert!(
        matches!(background.claim(&finished, Duration::ZERO), Claim::Unknown),
        "выдан один раз"
    );
}

#[test]
fn claim_waits_for_the_variant_being_compiled_and_times_out_when_it_does_not_come() {
    let Some(context) = gpu_context_or_skip() else { return };
    let cache = new_cache(48);
    let (job, gate) = gated_job(&cache, &context);
    let background = BackgroundPrewarm::spawn(job).unwrap();
    let busy = key(ToneBaseKind::Agx);
    background.enqueue([busy]);
    assert_eq!(
        gate.entered.recv_timeout(PATIENCE).unwrap(),
        busy,
        "фон начал компиляцию"
    );
    // Лимит короче, чем компиляция удерживается воротами: кадр уходит компилировать сам.
    assert!(matches!(
        background.claim(&busy, Duration::from_millis(30)),
        Claim::TimedOut
    ));
    // Ворота открываются из другого потока: кадр дожидается именно этой компиляции.
    let release = gate.release.clone();
    let opener = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        release.send(()).unwrap();
    });
    match background.claim(&busy, PATIENCE) {
        Claim::Delivered(result) => assert!(result.outcome.is_ok()),
        _ => panic!("кадр обязан дождаться компиляции"),
    }
    opener.join().unwrap();
}

#[test]
fn a_panicking_job_is_reported_once_and_stops_the_queue_without_touching_the_caller() {
    let Some(context) = gpu_context_or_skip() else { return };
    let cache = new_cache(48);
    let broken = key(ToneBaseKind::Agx);
    let hook: Hook = Arc::new(move |variant| {
        assert!(*variant != broken, "тестовая паника создания");
        Ok(())
    });
    let (background, driver) = BackgroundPrewarm::manual(job_with(&cache, &context, hook));
    background.enqueue([key(ToneBaseKind::Legacy), broken, key(ToneBaseKind::Filmic)]);
    assert_eq!(driver.run_all(), 2, "после паники очередь остановлена");
    let batch = background.take_ready();
    assert_eq!(batch.results.len(), 1, "до паники успели одного");
    let reason = batch.aborted.expect("паника обязана быть доложена");
    assert!(reason.contains("тестовая паника создания"), "{reason}");
    assert!(background.take_ready().aborted.is_none(), "причина выдаётся один раз");
    assert_eq!(
        background.enqueue([key(ToneBaseKind::Aces)]),
        0,
        "после аварии заданий не принимает"
    );
}

#[test]
fn dropping_mid_compile_waits_for_that_one_job_and_leaks_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let cache = new_cache(48);
        let (job, gate) = gated_job(&cache, &context);
        let background = BackgroundPrewarm::spawn(job).unwrap();
        background.enqueue([
            key(ToneBaseKind::Legacy),
            key(ToneBaseKind::Agx),
            key(ToneBaseKind::Filmic),
        ]);
        gate.entered.recv_timeout(PATIENCE).unwrap();
        let release = gate.release.clone();
        let opener = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            release.send(()).unwrap();
        });
        // Drop дожидается текущей компиляции и выходит; остальные задания отменены.
        drop(background);
        opener.join().unwrap();
        assert!(
            gate.entered.try_recv().is_err(),
            "после остановки следующее задание не начато"
        );
    }
    assert_eq!(context.live_objects(), baseline);
    assert_eq!(
        context.validation_report().errors,
        0,
        "{:?}",
        context.validation_report()
    );
}

#[test]
fn a_refused_thread_is_a_typed_error_not_a_panic() {
    let Some(context) = gpu_context_or_skip() else { return };
    let cache = new_cache(48);
    let refusal: Box<SpawnWorker> = Box::new(|_work| Err(std::io::Error::other("ОС не дала поток")));
    let result = BackgroundPrewarm::spawn_with(plain_job(&cache, &context), refusal);
    assert!(result.is_err());
}

// ---------- кэш с фоном ----------

#[test]
fn first_frame_creates_exactly_one_variant_and_background_delivers_the_rest_between_frames() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    let (background, driver) = BackgroundPrewarm::manual(plain_job(&cache, &context));
    cache.attach_background(background, PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    let stats = cache.stats();
    assert_eq!(cache.len(), 1, "на потоке рендера создан только вариант первого кадра");
    assert_eq!(
        (stats.created_in_frame, stats.created_by_prewarm, stats.last_prewarm),
        (1, 0, Duration::ZERO)
    );
    assert_eq!(
        driver.queued(),
        VARIANTS_PER_STAGE_SET - 1,
        "остальное семейство ушло в очередь фона"
    );
    // Фон отработал; кэш подберёт готовое на ближайшем кадре, без ожидания.
    assert_eq!(driver.run_all(), VARIANTS_PER_STAGE_SET - 1);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 2);
    let stats = cache.stats();
    assert_eq!(cache.len(), VARIANTS_PER_STAGE_SET);
    assert_eq!(
        (
            stats.created_in_background,
            stats.created_by_prewarm,
            stats.created_in_frame
        ),
        (VARIANTS_PER_STAGE_SET as u64 - 1, VARIANTS_PER_STAGE_SET as u64 - 1, 1)
    );
    assert_eq!(stats.background_discarded, 0);
    assert!(
        stats.background_drained_after.is_some(),
        "время готовности семейства измерено"
    );
    // Любой сосед теперь — попадание: ни одного создания в кадре.
    prepare(&mut cache, &context, key(ToneBaseKind::Filmic), 3);
    assert_eq!(cache.stats().created_in_frame, 1);
}

#[test]
fn a_frame_needing_a_queued_variant_compiles_it_once_and_the_background_does_not_repeat_it() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    let (background, driver) = BackgroundPrewarm::manual(plain_job(&cache, &context));
    cache.attach_background(background, PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    // Второй кадр — другая основа, фон ещё не сделал НИЧЕГО.
    prepare(&mut cache, &context, key(ToneBaseKind::Filmic), 2);
    let stats = cache.stats();
    assert_eq!(
        (stats.created_in_frame, stats.created_in_frame_while_background_pending),
        (2, 1)
    );
    assert_eq!(
        driver.queued(),
        VARIANTS_PER_STAGE_SET - 2,
        "этот ключ вынут из очереди"
    );
    driver.run_all();
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 3);
    let stats = cache.stats();
    assert_eq!(cache.len(), VARIANTS_PER_STAGE_SET);
    assert_eq!(
        stats.created_in_background,
        VARIANTS_PER_STAGE_SET as u64 - 2,
        "ни одной двойной компиляции"
    );
    assert_eq!(stats.background_discarded, 0);
}

#[test]
fn a_frame_needing_the_variant_being_compiled_waits_for_it_instead_of_compiling_twice() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    let (job, gate) = gated_job(&cache, &context);
    cache.attach_background(BackgroundPrewarm::spawn(job).unwrap(), PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    // Фон взял голову очереди — самый срочный вариант семейства.
    let urgent = gate.entered.recv_timeout(PATIENCE).unwrap();
    let release = gate.release.clone();
    let opener = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        release.send(()).unwrap();
    });
    prepare(&mut cache, &context, urgent, 2);
    opener.join().unwrap();
    let stats = cache.stats();
    assert_eq!(stats.created_in_frame, 1, "кадр сам компилировал только первый ключ");
    assert_eq!(stats.waited_for_background, 1);
    assert!(stats.longest_background_wait >= Duration::from_millis(20), "{stats:?}");
    assert!(cache.len() >= 2);
    // Остальные задания отпускаем и дожидаемся конца, чтобы Drop был быстрым.
    for _ in 0..VARIANTS_PER_STAGE_SET {
        let _ = gate.release.send(());
    }
}

#[test]
fn a_panic_in_the_background_degrades_to_on_demand_creation_and_frames_keep_going() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    let hook: Hook = Arc::new(|_| panic!("тестовая паника фона"));
    let (background, driver) = BackgroundPrewarm::manual(job_with(&cache, &context, hook));
    cache.attach_background(background, PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    driver.run_next();
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 2);
    let stats = cache.stats();
    assert!(stats.background_aborted);
    // Дальше — как при Off: вариант создаётся в кадре, заготовки нет, ничего не падает.
    prepare(&mut cache, &context, key(ToneBaseKind::Filmic), 3);
    let stats = cache.stats();
    assert_eq!(
        (cache.len(), stats.created_in_frame, stats.created_by_prewarm),
        (2, 2, 0)
    );
}

#[test]
fn a_failed_background_job_is_counted_and_the_variant_is_created_on_demand() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    let refused = key(ToneBaseKind::Filmic);
    let hook: Hook = Arc::new(move |variant| {
        if *variant == refused {
            Err(ViewfinderError::InvalidConfig {
                what: "тестовый отказ".to_owned(),
            })
        } else {
            Ok(())
        }
    });
    let (background, driver) = BackgroundPrewarm::manual(job_with(&cache, &context, hook));
    cache.attach_background(background, PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    driver.run_all();
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 2);
    let stats = cache.stats();
    assert_eq!(stats.prewarm_failed, 1);
    assert_eq!(cache.len(), VARIANTS_PER_STAGE_SET - 1);
    assert!(!stats.background_aborted, "отказ задания — не авария потока");
}

#[test]
fn without_a_thread_in_background_mode_the_family_is_prewarmed_on_the_render_thread() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(48);
    cache.note_background_unavailable();
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    let stats = cache.stats();
    assert_eq!(
        cache.len(),
        VARIANTS_PER_STAGE_SET,
        "запасной путь: семейство целиком в кадре"
    );
    assert!(stats.background_unavailable);
    assert_eq!(
        (stats.created_in_background, stats.created_by_prewarm),
        (0, VARIANTS_PER_STAGE_SET as u64 - 1)
    );
}

#[test]
fn background_results_for_a_full_cache_are_discarded_and_counted_not_evicting() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut cache = new_cache(2);
    let (background, driver) = BackgroundPrewarm::manual(plain_job(&cache, &context));
    cache.attach_background(background, PATIENCE);
    prepare(&mut cache, &context, key(ToneBaseKind::Agx), 1);
    assert_eq!(driver.queued(), 1, "в очередь только то, что поместится");
    // Кадр занял второе место сам до того, как фон закончил: готовое ему уже некуда.
    prepare(&mut cache, &context, key(ToneBaseKind::Filmic), 2);
    driver.run_all();
    assert_eq!(cache.adopt_prewarmed(), 0);
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.stats().evicted, 0, "заготовка никого не вытесняет");
    assert_eq!(
        cache.stats().background_discarded,
        1,
        "готовое некуда положить: уничтожено и посчитано"
    );
}
