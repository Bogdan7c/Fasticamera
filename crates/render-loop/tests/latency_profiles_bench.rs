//! ПК-стенд сессии 07 (P13 → P14): проверка гипотез о задержке и плавности видоискателя ДО телефона.
//!
//! Настоящий поток рендера + `VulkanDisplayBackend` камеры на RADV (headless-показ, validation FULL) +
//! настоящий `RawFrameStream`. Синтетика вместо платформы:
//! * vsync 60,00 Гц с линиями кадра как у Choreographer: линия `i` — срок `t + (i+1)·P − 2 мс`, показ
//!   `t + (i+2)·P`, предпочитаемая — индекс 1 (показ на 3-м vsync, как на OnePlus 11);
//! * камера 60,03 к/с: метка сенсора по своей шкале, ПРИХОД кадра = метка + возраст, возраст равномерно
//!   в [27; 43] мс (факт телефона: «возраст при приходе» 27–43 мс), детерминированный генератор.
//!
//! Профили по одинаковой последовательности дрожания (P14, решения 53 и 54):
//! * `baseline` — как до P14: `Preferred`, без позднего захвата, без схода, `LatestWins`, ADPF выключен;
//! * `tuned` — продукт после P14 (= значения по умолчанию, проверяется тестом): `NearestFeasible{1,5 мс}`,
//!   сход на ближнюю линию пропуском vsync, буфер видоискателя {запас 1, очередь 2}, поздний захват,
//!   ADPF включён (на ПК — подделка службы);
//! * `no_latch` — `tuned` без позднего захвата: отвечает, нужен ли захват при буфере видоискателя.
//!
//! Меряется по окну: кадры, показанные НЕ ровно один раз (сброшенные: вытеснены `LatestWins` или
//! переполнили буфер; повторённые: тик показал прежний кадр или пропущен ради схода), задержка
//! «сенсор → задуманный показ» (метка сенсора → `expected_present` выбранной линии в момент опроса),
//! возраст кадра при опросе, сходы и пропущенные тики.
//!
//! Чего ПК не воспроизводит: защёлку буфера SurfaceFlinger и настоящие времена показа
//! (`VK_GOOGLE_display_timing` нет — «задуманный показ» не проверяется фактом), DVFS Adreno (GPU RADV
//! тратит доли мс, а не ~13), ADPF (подделка), конкуренцию за ядра на телефоне. Поэтому стенд
//! отвечает только на вопрос «что меняет политика при данном дрожании прихода», а не «сколько к/с
//! будет на телефоне».
//!
//! Запуск с выводом чисел: `cargo test -p render-loop --release --test latency_profiles_bench -- --nocapture`;
//! длина окна — `FASTICAMERA_BENCH_WINDOW_MS` (по умолчанию 3000).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod camera_rig;
mod latency_bench_platform;

use latency_bench_platform::{
    JITTER_SEED, Jitter, JitteredCamera, VSYNC_PERIOD_NANOS, monotonic_now, vsync_clock_60_hz,
};
mod support;
mod vulkan_support;

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self};
use std::time::{Duration, Instant};

use camera_rig::{EventLog, ProducerBook, SURFACE_SIZE, TestImporter};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use raw_frame_stream::{RawFrameLease, RawFrameStream, RawFrameStreamConfig, RawStreamCounters, ViewfinderPacing};
use render_loop::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameCpuTiming, FrameGpuTiming, FrameTiming,
    PastPresentation, PresentOutcome,
};
use render_loop::config::{
    AdpfConfig, AdpfMode, DepthReductionMode, LateLatchMode, PresentTargetConfig, PresentTimelinePolicy,
    RenderLoopConfig,
};
use render_loop::thread::{GpuCostSource, RenderLoop, RenderLoopHandle, RenderLoopParts, RenderStatus};
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use render_loop::vulkan_backend::{CameraFrameImporter, CameraImportError, SubmissionSerial};
use shell_contract::performance_hint::fake::FakePerformanceHints;
use shell_contract::performance_hint::{NoPerformanceHints, PerformanceHintService};
use shell_contract::surface::{
    NativeSurface, SurfaceGeneration, SurfaceReleaseOutcome, SurfaceSize, surface_release_handoff,
};
use support::{FakeCommands, Journal as WindowJournal, fake_surface, wait_until};
use vulkan_support::{assert_no_validation_errors, headless_camera_backend_or_skip};

// ---- параметры окна ----

/// Разогрев окна, не меряется: оценки стоимости набирают замеры GPU (~0,5 с), серия схода — ещё
/// 30 кадров (~0,5 с), буфер видоискателя заполняется. 2 с — с запасом вдвое.
const WARMUP: Duration = Duration::from_millis(2_000);
const DEFAULT_WINDOW_MS: u64 = 3_000;
const DEADLINE: Duration = Duration::from_secs(20);

// ---- шпион: план кадра и возраст кадра при опросе ----

/// Что знает шпион: план текущего кадра (ставит бэкенд-обёртка) и замеры импортёра.
#[derive(Default)]
struct Spy {
    current_target_expected: Mutex<Option<i64>>,
    samples: Mutex<Vec<AgeSample>>,
}

#[derive(Debug, Clone, Copy)]
struct AgeSample {
    /// Метка сенсора → опрос (импорт идёт сразу после опроса видоискателя), нс.
    age_at_poll: i64,
    /// Метка сенсора → `expected_present` выбранной линии, нс.
    sensor_to_planned_present: i64,
}

impl Spy {
    fn lock_samples(&self) -> std::sync::MutexGuard<'_, Vec<AgeSample>> {
        self.samples.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Бэкенд-обёртка: запоминает `expected_present` плана перед кадром, остальное — как есть.
struct SpyBackend<B: DisplayBackend> {
    inner: B,
    spy: Arc<Spy>,
}

impl<B: DisplayBackend> DisplayBackend for SpyBackend<B> {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        self.inner.attach(surface, size)
    }
    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        self.inner.resize(size)
    }
    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        *self
            .spy
            .current_target_expected
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(timing.expected_present().as_nanos());
        self.inner.render_and_present(timing)
    }
    fn detach(&mut self) -> Option<NativeSurface> {
        self.inner.detach()
    }
    fn diagnostics(&mut self) -> BackendDiagnostics {
        self.inner.diagnostics()
    }
    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        self.inner.drain_past_presentations()
    }
    fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        self.inner.take_frame_cpu_timing()
    }
    fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        self.inner.drain_frame_gpu_timings()
    }
    fn apply_viewfinder_command(&mut self, command: ViewfinderCommand) -> ViewfinderCommandOutcome {
        self.inner.apply_viewfinder_command(command)
    }
}

/// Импортёр-обёртка: на ПЕРВОМ импорте кадра пишет его возраст (импорт — сразу после опроса).
struct SpyImporter {
    inner: TestImporter,
    spy: Arc<Spy>,
}

impl CameraFrameImporter for SpyImporter {
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError> {
        let sensor_ts = lease.passport().sensor_timestamp.as_nanos();
        let now = monotonic_now();
        if !self.inner.entries.contains_key(&sensor_ts) {
            let expected = *self
                .spy
                .current_target_expected
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(expected) = expected {
                self.spy.lock_samples().push(AgeSample {
                    age_at_poll: now - sensor_ts,
                    sensor_to_planned_present: expected - sensor_ts,
                });
            }
        }
        self.inner.import_frame(lease, submission)
    }
    fn retire_completed(&mut self, completed: SubmissionSerial) {
        self.inner.retire_completed(completed);
    }
    fn release_all_imports_gpu_idle(&mut self) {
        self.inner.release_all_imports_gpu_idle();
    }
    fn imports_alive(&self) -> usize {
        self.inner.imports_alive()
    }
    fn cache_counters(&self) -> ImportCacheCounters {
        self.inner.cache_counters()
    }
}

// ---- профили и окно ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Baseline,
    Tuned,
    /// `tuned` без позднего захвата: нужен ли захват при буфере видоискателя.
    TunedWithoutLatch,
}

impl Profile {
    fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Tuned => "tuned",
            Self::TunedWithoutLatch => "no_latch",
        }
    }

    fn present_target(self) -> PresentTargetConfig {
        match self {
            Self::Baseline => PresentTargetConfig {
                policy: PresentTimelinePolicy::Preferred,
                late_latch: LateLatchMode::Disabled,
                depth_reduction: DepthReductionMode::Disabled,
                ..PresentTargetConfig::default()
            },
            Self::Tuned => PresentTargetConfig {
                policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 },
                late_latch: LateLatchMode::BeforeCameraPoll,
                depth_reduction: DepthReductionMode::SkipOneVsync,
                ..PresentTargetConfig::default()
            },
            Self::TunedWithoutLatch => PresentTargetConfig {
                late_latch: LateLatchMode::Disabled,
                ..Self::Tuned.present_target()
            },
        }
    }

    fn viewfinder_pacing(self) -> ViewfinderPacing {
        match self {
            Self::Baseline => ViewfinderPacing::LatestWins,
            Self::Tuned | Self::TunedWithoutLatch => ViewfinderPacing::JitterBuffer,
        }
    }

    fn adpf(self) -> AdpfConfig {
        AdpfConfig {
            mode: match self {
                Self::Baseline => AdpfMode::Disabled,
                Self::Tuned | Self::TunedWithoutLatch => AdpfMode::Enabled,
            },
            ..AdpfConfig::default()
        }
    }
}

/// Итог одного окна.
#[derive(Debug, Clone, Copy)]
struct WindowResult {
    profile: Profile,
    /// Фаза камеры к vsync, доля периода.
    phase: f64,
    seconds: f64,
    offered: u64,
    /// Сброшены: вытеснены `LatestWins` + переполнили буфер видоискателя.
    superseded: u64,
    overflow: u64,
    /// Повторы на экране: показанный кадр экрана без нового кадра камеры + тики, пропущенные ради схода.
    repeats: u64,
    camera_presented_per_s: f64,
    display_frames_per_s: f64,
    age_at_poll_p50_ms: f64,
    age_at_poll_p95_ms: f64,
    planned_p50_ms: f64,
    planned_p95_ms: f64,
    gpu_source: GpuCostSource,
    estimated_cost_ms: f64,
    fallbacks: u64,
    latch_refusals: u64,
    adpf_reports: u64,
    /// Сходы на ближнюю линию за всё окно потока (включая разогрев) и пропуски тиков в самом окне.
    depth_reductions_total: u64,
    skipped_ticks: u64,
}

impl WindowResult {
    fn superseded_ratio(&self) -> f64 {
        self.superseded as f64 / self.offered.max(1) as f64
    }

    /// Доля кадров камеры, показанных НЕ ровно один раз: (сброшенные + повторы) / предложенные.
    fn judder_ratio(&self) -> f64 {
        (self.superseded + self.overflow + self.repeats) as f64 / self.offered.max(1) as f64
    }

    fn line(&self) -> String {
        format!(
            "{:<8} фаза {:.2} {:>4.1} с: предложено {:>4}, вытеснено {:>3}, переполнение {:>2}, повторов {:>3} \
             (не ровно один раз {:>5.2} %), новых кадров камеры {:>5.2}/с, кадров экрана {:>5.2}/с, возраст при \
             опросе p50 {:>5.2} / p95 {:>5.2} мс, сенсор→задуманный показ p50 {:>5.2} / p95 {:>5.2} мс, GPU в \
             оценке {:?}, оценка {:.2} мс, откатов {}, отказов ждать {}, отчётов ADPF {}, сходов (с разогревом) {}, \
             пропусков в окне {}",
            self.profile.name(),
            self.phase,
            self.seconds,
            self.offered,
            self.superseded,
            self.overflow,
            self.repeats,
            self.judder_ratio() * 100.0,
            self.camera_presented_per_s,
            self.display_frames_per_s,
            self.age_at_poll_p50_ms,
            self.age_at_poll_p95_ms,
            self.planned_p50_ms,
            self.planned_p95_ms,
            self.gpu_source,
            self.estimated_cost_ms,
            self.fallbacks,
            self.latch_refusals,
            self.adpf_reports,
            self.depth_reductions_total,
            self.skipped_ticks,
        )
    }
}

fn percentile_ms(values: &mut [i64], quantile: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_unstable();
    let rank = ((quantile * values.len() as f64).ceil() as usize).clamp(1, values.len());
    values[rank - 1] as f64 / 1e6
}

fn window_ms() -> u64 {
    std::env::var("FASTICAMERA_BENCH_WINDOW_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_WINDOW_MS)
}

/// Снимок «до» окна.
struct Mark {
    at: Instant,
    counters: RawStreamCounters,
    status: RenderStatus,
    samples: usize,
}

fn mark(handle: &RenderLoopHandle, stream: &RawFrameStream, spy: &Spy) -> Mark {
    Mark {
        at: Instant::now(),
        counters: stream.counters(),
        status: handle.status(),
        samples: spy.lock_samples().len(),
    }
}

fn camera_presented(status: &RenderStatus) -> u64 {
    status
        .backend
        .camera
        .as_ref()
        .map_or(0, |camera| camera.camera_frames_presented)
}

/// Одно окно профиля на СВОЁМ потоке рендера и своём GPU-контексте (чистое начало каждого окна).
fn run_window(profile: Profile, phase: f64) -> Option<WindowResult> {
    let config = RenderLoopConfig {
        preferred_frame_rate_hz: 60.0,
        diagnostics_poll_interval_ms: 50,
        present_target: profile.present_target(),
        adpf: profile.adpf(),
        ..RenderLoopConfig::default()
    };
    let stream = RawFrameStream::new(RawFrameStreamConfig {
        viewfinder_pacing: profile.viewfinder_pacing(),
        ..RawFrameStreamConfig::default()
    });
    let log = Arc::new(EventLog::default());
    let book = Arc::new(ProducerBook::default());
    let spy = Arc::new(Spy::default());
    let (factory_log, factory_spy) = (Arc::clone(&log), Arc::clone(&spy));
    let backend = headless_camera_backend_or_skip(
        &config,
        stream.viewfinder(),
        Box::new(move |context: &GpuContext| {
            Ok(Box::new(SpyImporter {
                inner: TestImporter {
                    context: context.clone(),
                    log: Arc::clone(&factory_log),
                    alive: Arc::new(AtomicUsize::new(0)),
                    entries: HashMap::new(),
                    counters: ImportCacheCounters::default(),
                },
                spy: Arc::clone(&factory_spy),
            }))
        }),
    )?;
    let context = backend.context().clone();
    let hints: Arc<dyn PerformanceHintService> = match profile {
        Profile::Baseline => Arc::new(NoPerformanceHints::new("ПК-стенд")),
        Profile::Tuned | Profile::TunedWithoutLatch => Arc::new(FakePerformanceHints::new(1)),
    };
    // Общий якорь vsync и камеры чуть в будущем: оба потока успевают стартовать до первого момента.
    let anchor = monotonic_now() + 100_000_000;
    let handle = RenderLoop::spawn(RenderLoopParts {
        config,
        backend: SpyBackend {
            inner: backend,
            spy: Arc::clone(&spy),
        },
        clock: vsync_clock_60_hz(anchor),
        commands: Arc::new(FakeCommands::default()),
        performance_hints: hints,
    })
    .expect("поток рендера");
    let windows = Arc::new(WindowJournal::default());
    let inbox = handle.inbox();
    inbox.surface_created(fake_surface(&windows, 1));
    inbox.surface_changed(SurfaceGeneration::new(1), SURFACE_SIZE);
    let phase_nanos = (phase * VSYNC_PERIOD_NANOS as f64).round() as i64;
    let mut camera = JitteredCamera::start(&stream, &log, &book, anchor, phase_nanos);
    assert!(
        wait_until(DEADLINE, || camera_presented(&handle.status()) >= 30),
        "кадры камеры не дошли до показа: {:?}",
        handle.status()
    );
    thread::sleep(WARMUP);
    let before = mark(&handle, &stream, &spy);
    thread::sleep(Duration::from_millis(window_ms()));
    let after = mark(&handle, &stream, &spy);
    camera.stop();

    let seconds = after.at.duration_since(before.at).as_secs_f64();
    let samples: Vec<AgeSample> = spy.lock_samples()[before.samples..after.samples].to_vec();
    let mut ages: Vec<i64> = samples.iter().map(|sample| sample.age_at_poll).collect();
    let mut planned: Vec<i64> = samples.iter().map(|sample| sample.sensor_to_planned_present).collect();
    let target = &after.status.present_target;
    let previous_target = &before.status.present_target;
    let counter_delta = |field: fn(&RawStreamCounters) -> u64| field(&after.counters) - field(&before.counters);
    let display_frames = after.status.frames_presented - before.status.frames_presented;
    let camera_frames = camera_presented(&after.status) - camera_presented(&before.status);
    let skipped_ticks = target.skipped_ticks_for_depth - previous_target.skipped_ticks_for_depth;
    let result = WindowResult {
        profile,
        phase,
        seconds,
        offered: counter_delta(|counters| counters.frames_offered),
        superseded: counter_delta(|counters| counters.dropped_superseded),
        overflow: counter_delta(|counters| counters.dropped_jitter_overflow),
        repeats: display_frames.saturating_sub(camera_frames) + skipped_ticks,
        camera_presented_per_s: camera_frames as f64 / seconds,
        display_frames_per_s: display_frames as f64 / seconds,
        age_at_poll_p50_ms: percentile_ms(&mut ages, 0.5),
        age_at_poll_p95_ms: percentile_ms(&mut ages, 0.95),
        planned_p50_ms: percentile_ms(&mut planned, 0.5),
        planned_p95_ms: percentile_ms(&mut planned, 0.95),
        gpu_source: target.estimated_cost_parts.gpu_source,
        estimated_cost_ms: target.estimated_frame_cost.as_secs_f64() * 1e3,
        fallbacks: target.fallbacks_to_preferred - previous_target.fallbacks_to_preferred,
        latch_refusals: target.latch_waits_refused - previous_target.latch_waits_refused,
        adpf_reports: after.status.adpf.reports_sent - before.status.adpf.reports_sent,
        depth_reductions_total: target.depth_reductions,
        skipped_ticks,
    };

    // Уборка: surface отпускается, поток останавливается; ничего не утекло, validation чиста.
    let (request, waiter) = surface_release_handoff(SurfaceGeneration::new(1));
    handle.inbox().surface_destroyed(request);
    assert_eq!(waiter.wait(DEADLINE), SurfaceReleaseOutcome::Confirmed);
    let final_status = handle.stop();
    assert_eq!(
        stream.counters().leaked_leases,
        0,
        "{}: утечка лизингов",
        profile.name()
    );
    assert_eq!(
        final_status.backend.validation_errors,
        0,
        "{}: ошибки validation",
        profile.name()
    );
    assert_no_validation_errors(&context);
    Some(result)
}

/// Среднее поля по окнам профиля — по всем фазам камеры к vsync (на телефоне фаза дрейфует: 60,03
/// против 60,00 Гц — полный оборот за ~33 с, поэтому честное сравнение — среднее по фазам).
fn mean_of(results: &[WindowResult], profile: Profile, field: impl Fn(&WindowResult) -> f64) -> f64 {
    let values: Vec<f64> = results
        .iter()
        .filter(|result| result.profile == profile)
        .map(field)
        .collect();
    values.iter().sum::<f64>() / values.len().max(1) as f64
}

/// Фазы камеры к vsync, доли периода: четыре точки оборота дрейфа.
const CAMERA_PHASES: [f64; 4] = [0.0, 0.25, 0.5, 0.75];

/// Порог «не ровно один раз» для `tuned`, доля кадров (выбран ДО прогона, не подогнан): дрейф часов
/// 60,03/60,00 даёт 1 кадр за ~33 с (0,05 %), сход — один повтор и один сброс за поток (вне окна:
/// разогрев 2 с), остальное — дрожание пробуждений потоков на ПК (сон ОС опаздывает на доли мс, опрос
/// с поздним захватом встаёт близко к приходу кадра). 2 % — на порядок ниже ~16 % `LatestWins`.
const TUNED_JUDDER_LIMIT: f64 = 0.02;

/// Итог стенда P14 (числа печатаются строками `BENCH`, `--nocapture`).
///
/// Окна идут по фазам камеры к vsync (на телефоне фаза дрейфует, полный оборот ~33 с), внутри фазы
/// профили чередуются (A B C, C B A); сравниваются средние по фазам.
/// 1. ПЛАВНОСТЬ: `baseline` (`LatestWins`) теряет заметную долю кадров при этом дрожании (P13: ~16 %);
///    `tuned` (буфер видоискателя) показывает каждый кадр ровно один раз с точностью до
///    [`TUNED_JUDDER_LIMIT`]; в каждом окне `tuned` нет вытеснений «самым свежим».
/// 2. ЗАДЕРЖКА «сенсор → задуманный показ» p50: `tuned` не хуже `baseline` (буфер стоит около кадра,
///    сход на ближнюю линию возвращает период). Выигрыш ≥ 0,5 периода печатается как вердикт, но не
///    закрепляется: он зависит от глубины, на которую встаёт `baseline`, а не только от нас.
/// 3. Сход работает: в каждом окне `tuned` поток сошёл на ближнюю линию хотя бы раз.
/// 4. Цепочка P13 работает: оценка стоимости опирается на замеры GPU, ADPF (подделка) получает отчёты.
///
/// Утечки лизингов и ошибки validation проверяются в каждом окне (`run_window`).
#[test]
fn tuned_viewfinder_against_baseline_on_jittered_camera() {
    let profiles = [Profile::Baseline, Profile::Tuned, Profile::TunedWithoutLatch];
    let mut results = Vec::new();
    for (index, phase) in CAMERA_PHASES.into_iter().enumerate() {
        let mut order = profiles;
        if index % 2 == 1 {
            order.reverse();
        }
        for profile in order {
            let Some(result) = run_window(profile, phase) else {
                return;
            };
            println!("BENCH {}", result.line());
            results.push(result);
        }
    }
    let judder = |profile| mean_of(&results, profile, WindowResult::judder_ratio);
    let superseded = |profile| mean_of(&results, profile, WindowResult::superseded_ratio);
    let planned = |profile| mean_of(&results, profile, |result| result.planned_p50_ms);
    let planned_p95 = |profile| mean_of(&results, profile, |result| result.planned_p95_ms);
    let age = |profile| mean_of(&results, profile, |result| result.age_at_poll_p50_ms);
    let fps = |profile| mean_of(&results, profile, |result| result.camera_presented_per_s);
    for profile in profiles {
        println!(
            "BENCH среднее по фазам {:<8}: не ровно один раз {:>5.2} % (вытеснено {:>5.2} %), новых кадров камеры \
             {:>5.2}/с, возраст при опросе p50 {:>5.2} мс, сенсор→задуманный показ p50 {:>5.2} / p95 {:>5.2} мс",
            profile.name(),
            judder(profile) * 100.0,
            superseded(profile) * 100.0,
            fps(profile),
            age(profile),
            planned(profile),
            planned_p95(profile),
        );
    }
    let period_ms = VSYNC_PERIOD_NANOS as f64 / 1e6;
    let gain_periods = (planned(Profile::Baseline) - planned(Profile::Tuned)) / period_ms;
    println!(
        "BENCH вердикт: tuned быстрее baseline на {:.2} периода (цель ≥ 0,5: {}), поздний захват даёт {:.2} мс",
        gain_periods,
        if gain_periods >= 0.5 {
            "достигнута"
        } else {
            "НЕ достигнута"
        },
        planned(Profile::TunedWithoutLatch) - planned(Profile::Tuned),
    );

    // 4. Цепочка P13 работает.
    for result in results.iter().filter(|result| result.profile != Profile::Baseline) {
        assert_eq!(result.gpu_source, GpuCostSource::Measured, "{}", result.line());
        assert!(result.adpf_reports > 0, "{}", result.line());
        // 3. Сход случился (за разогрев), 1. и вытеснений «самым свежим» у буфера нет.
        assert!(
            result.depth_reductions_total >= 1,
            "сход не случился: {}",
            result.line()
        );
        assert_eq!(result.superseded, 0, "буфер не вытесняет: {}", result.line());
    }
    // 1. Плавность.
    assert!(
        superseded(Profile::Baseline) > 0.05,
        "baseline больше не теряет кадры при этом дрожании — стенд не воспроизводит проблему"
    );
    for profile in [Profile::Tuned, Profile::TunedWithoutLatch] {
        assert!(
            judder(profile) <= TUNED_JUDDER_LIMIT,
            "{}: не ровно один раз {:.2} % > {:.0} %",
            profile.name(),
            judder(profile) * 100.0,
            TUNED_JUDDER_LIMIT * 100.0
        );
    }
    // 2. Задержка не хуже.
    assert!(
        planned(Profile::Tuned) <= planned(Profile::Baseline),
        "tuned медленнее baseline: {:.2} против {:.2} мс",
        planned(Profile::Tuned),
        planned(Profile::Baseline)
    );
}

/// Стенд меряет ровно то, что продукт делает по умолчанию: `tuned` = значения по умолчанию.
#[test]
fn tuned_profile_is_the_product_default() {
    assert_eq!(Profile::Tuned.present_target(), PresentTargetConfig::default());
    assert_eq!(
        Profile::Tuned.viewfinder_pacing(),
        RawFrameStreamConfig::default().viewfinder_pacing
    );
}

/// Не даёт тесту забыть про сам стенд: доля вытеснений считается по одной последовательности дрожания.
#[test]
fn jitter_is_deterministic_and_within_the_phone_range() {
    let mut first = Jitter(JITTER_SEED);
    let mut second = Jitter(JITTER_SEED);
    let ages: Vec<i64> = (0..10_000).map(|_| first.arrival_age_nanos()).collect();
    assert!(ages.iter().all(|age| (27_000_000..=43_000_000).contains(age)));
    assert_eq!(
        ages[..10],
        (0..10).map(|_| second.arrival_age_nanos()).collect::<Vec<_>>()[..]
    );
    let mean = ages.iter().sum::<i64>() as f64 / ages.len() as f64;
    assert!((mean - 35e6).abs() < 0.3e6, "среднее {mean}");
}
