//! Снимок под каждой моделью основы тона на настоящем GPU ПК (RADV): модель доходит до картинки, а оба пути
//! выходного прохода (`per_model` — вариант пайплайна на модель, `runtime` — прежний путь) дают ОДНУ картинку.
//!
//! Те же работы гоняются на телефоне (`still_<модель>_<путь>`): там сравнение отпечатков — доказательство,
//! что компилятор Adreno не изменил младшие биты. Печатает таблицу отпечатков (`-- --nocapture`).

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::collections::HashSet;
use std::time::Duration;

use camera_contract::lens_shading::LensShadingCalibrations;
use fasticamera::camera_screen_config::CameraScreenConfig;
use fasticamera::look_service::LookHandle;
use fasticamera::look_startup::catalog_environment;
use fasticamera::power_measurement::ThermalSnapshot;
use fasticamera::saturation_bench::catalog::{CatalogEntry, find_entry};
use fasticamera::saturation_bench::digest_report::DigestOutcome;
use fasticamera::saturation_bench::gpu_executor::GpuWorkloadExecutor;
use fasticamera::saturation_bench::inputs::{InputSources, ProductInputs, gather_inputs};
use fasticamera::saturation_bench::report::{RunSummary, WorkloadRow};
use fasticamera::saturation_bench::runner::{BenchRunner, PauseOutcome, RunClock, RunEvent, ThermalProbe};
use fasticamera::saturation_bench::screen_scene::SurfaceSizePx;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::saturation::{CancelToken, SaturationConfig};
use gpu_vulkan::swapchain::FramesInFlight;
use gpu_vulkan::validation::ValidationMode;
use look_presets::PresetStoreConfig;
use render_loop::config::RenderLoopConfig;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Модели основы тона в том же порядке, что имена работ каталога.
const MODELS: [&str; 4] = ["agx", "aces", "filmic", "legacy"];

/// Входы продукта без камеры: запасной сенсор и библиотека пресетов в памяти (вид по умолчанию, «Natural»).
fn product_inputs() -> ProductInputs {
    let render = RenderLoopConfig::default();
    let look = LookHandle::in_memory(&PresetStoreConfig::default(), &catalog_environment(&render)).expect("библиотека");
    let screen = CameraScreenConfig::default();
    let lens_shading = LensShadingCalibrations::none();
    gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &lens_shading,
        render_config: &render,
        look: Some(&look),
        screen: &screen,
    })
}

struct NoPause;

impl RunClock for NoPause {
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
    fn pause(&self, _duration: Duration, _cancel: &CancelToken) -> PauseOutcome {
        PauseOutcome::Elapsed
    }
}

struct NoThermal;

impl ThermalProbe for NoThermal {
    fn snapshot(&self) -> ThermalSnapshot {
        ThermalSnapshot {
            status: None,
            headroom: None,
            error: Some("на ПК теплового сервиса нет".to_owned()),
        }
    }
}

/// Прогон работ через раннер стенда (окно короткое: нужны отпечатки, а не цена).
fn run_entries(context: &GpuContext, entries: &[&'static CatalogEntry]) -> RunSummary {
    let bench = SaturationConfig {
        frames_in_flight: FramesInFlight::new(2).unwrap(),
        warmup: Duration::from_millis(100),
        measure: Duration::from_millis(300),
        timeline_step: Duration::from_millis(100),
        fence_timeout: Duration::from_secs(30),
    };
    let mut executor = GpuWorkloadExecutor::new(context.clone(), product_inputs(), bench, SurfaceSizePx::ONEPLUS_11);
    let mut observer = |_event: RunEvent| {};
    let summary = BenchRunner {
        executor: &mut executor,
        clock: &NoPause,
        thermal: &NoThermal,
        observer: &mut observer,
        cooldown_between: Duration::ZERO,
    }
    .run(entries, &CancelToken::new());
    drop(executor);
    summary
}

/// Отпечаток части выхода (`image` или `thumbnail`) строки; паника с причиной, если его нет.
fn digest_part(summary: &RunSummary, name: &str, label: &str) -> String {
    let row: &WorkloadRow = summary.rows.iter().find(|row| row.name == name).expect("строка работы");
    match row.digest.as_ref().unwrap_or_else(|| panic!("{name}: нет дайджеста")) {
        DigestOutcome::Computed { parts, .. } => parts
            .iter()
            .find(|part| part.label == label)
            .unwrap_or_else(|| panic!("{name}: нет части {label}"))
            .digest
            .clone(),
        DigestOutcome::Unavailable { reason } => panic!("{name}: дайджест недоступен: {reason}"),
    }
}

fn gpu_context_or_skip() -> Option<GpuContext> {
    match GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)) {
        Ok(context) => Some(context),
        Err(error) if std::env::var(SKIP_ENV).as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
            None
        }
        Err(error) => panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно"),
    }
}

#[test]
fn still_picture_is_bit_exact_between_output_paths_for_every_tone_model_and_differs_between_models() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut names: Vec<String> = vec!["still".to_owned()];
    for model in MODELS {
        names.push(format!("still_{model}_per_model"));
        names.push(format!("still_{model}_runtime"));
    }
    let entries: Vec<&'static CatalogEntry> = names.iter().map(|name| find_entry(name).expect("работа")).collect();
    let summary = run_entries(&context, &entries);
    assert_eq!(summary.rows.len(), entries.len());

    println!("| работа | image | thumbnail |");
    for name in &names {
        println!(
            "| {name} | {} | {} |",
            digest_part(&summary, name, "image"),
            digest_part(&summary, name, "thumbnail")
        );
    }
    for label in ["image", "thumbnail"] {
        for model in MODELS {
            assert_eq!(
                digest_part(&summary, &format!("still_{model}_per_model"), label),
                digest_part(&summary, &format!("still_{model}_runtime"), label),
                "{model}: {label} под вариантом на модель отличается от прежнего пути"
            );
        }
        let per_model: HashSet<String> = MODELS
            .iter()
            .map(|model| digest_part(&summary, &format!("still_{model}_per_model"), label))
            .collect();
        assert_eq!(
            per_model.len(),
            MODELS.len(),
            "{label}: модель не дошла до шейдера, выходы совпали"
        );
        assert_eq!(
            digest_part(&summary, "still_agx_per_model", label),
            digest_part(&summary, "still", label),
            "{label}: основа по умолчанию (AgX) должна совпадать с работой still"
        );
    }
}
