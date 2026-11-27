//! Группа стенда насыщения `scopes_pages` на настоящем GPU ПК (RADV, полная validation): страницы приборов сверх
//! `baseline` — LUMA WAVEFORM и гистограмма в трёх режимах (сессия 14s).
//!
//! Что проверяется (результат, а не вызов):
//! * у каждой работы есть цифра пропускной способности, кадры в окне > 0 и метки проходов накопления и показа;
//! * отпечаток выхода берёт и счётчики, и картинку; у гистограммы PARADE счётчики те же, что у RGB (одни и те же отсчёты,
//!   те же три плоскости), а картинка другая (секции вместо наложения); у режима Y счётчики другие (одна плоскость);
//! * после прогона живых объектов Vulkan столько же, сколько до него, validation не нашла ошибок.
//!
//! Печатает таблицу цены: `cargo test -p fasticamera-app --release --test saturation_scopes_pages -- --nocapture`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::time::Duration;

use camera_contract::lens_shading::LensShadingCalibrations;
use fasticamera::camera_screen_config::CameraScreenConfig;
use fasticamera::look_service::LookHandle;
use fasticamera::look_startup::catalog_environment;
use fasticamera::power_measurement::ThermalSnapshot;
use fasticamera::saturation_bench::catalog::{CatalogEntry, WorkloadGroup, group_entries};
use fasticamera::saturation_bench::digest_report::DigestOutcome;
use fasticamera::saturation_bench::gpu_executor::GpuWorkloadExecutor;
use fasticamera::saturation_bench::inputs::{InputSources, ProductInputs, gather_inputs};
use fasticamera::saturation_bench::report::{RowOutcome, RunSummary, WorkloadRow};
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

/// Окно замера: короткое по умолчанию (нужны отпечатки и метки), длиннее — для цифр цены (`FASTICAMERA_SCOPES_PAGES_MS`).
fn measure_ms() -> u64 {
    std::env::var("FASTICAMERA_SCOPES_PAGES_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300)
}

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

fn run_entries(context: &GpuContext, entries: &[&'static CatalogEntry]) -> RunSummary {
    let bench = SaturationConfig {
        frames_in_flight: FramesInFlight::new(2).unwrap(),
        warmup: Duration::from_millis(100),
        measure: Duration::from_millis(measure_ms()),
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

fn row<'a>(summary: &'a RunSummary, name: &str) -> &'a WorkloadRow {
    summary.rows.iter().find(|row| row.name == name).expect("строка работы")
}

/// Отпечаток части выхода строки по метке; паника с причиной, если отпечатка нет.
fn digest_part(summary: &RunSummary, name: &str, label: &str) -> String {
    match row(summary, name)
        .digest
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: у строки нет дайджеста"))
    {
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
fn the_scopes_pages_group_measures_luma_the_histogram_modes_and_both_cie_diagrams_with_distinct_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let entries = group_entries(WorkloadGroup::ScopesPages);
    let names: Vec<&str> = entries.iter().map(|entry| entry.name).collect();
    assert_eq!(
        names,
        [
            "scopes_luma_waveform",
            "scopes_histogram_rgb",
            "scopes_histogram_parade",
            "scopes_histogram_luma",
            "scopes_cie_xy",
            "scopes_cie_uv"
        ]
    );

    let summary = run_entries(&context, &entries);

    println!(
        "\nСтраницы приборов на стенде насыщения ({}, окно {} мс)",
        context.capabilities().device_name,
        measure_ms()
    );
    for row in &summary.rows {
        let RowOutcome::Measured { measurement } = &row.outcome else {
            panic!("{}: нет цифры: {:?}", row.name, row.outcome);
        };
        let passes: Vec<String> = measurement
            .passes
            .iter()
            .map(|pass| format!("{} {:.3} мс", pass.label, pass.median_ms))
            .collect();
        println!(
            "| {} | {:.1} к/с | {} |",
            row.name,
            measurement.frames_per_second.unwrap(),
            passes.join("; ")
        );
        assert!(measurement.measured_frames > 0, "{}: в окне нет кадров", row.name);
        let labels: Vec<&str> = measurement.passes.iter().map(|pass| pass.label).collect();
        assert_eq!(labels, ["accumulate-end", "render-end"], "{}: метки проходов", row.name);
    }

    // Отпечатки: те же отсчёты и плоскости — те же счётчики; другой способ рисования — другая картинка.
    let counters = |name| digest_part(&summary, name, "counters");
    let picture = |name| digest_part(&summary, name, "picture");
    assert_eq!(
        counters("scopes_histogram_parade"),
        counters("scopes_histogram_rgb"),
        "PARADE и RGB считают одно и то же"
    );
    assert_ne!(
        picture("scopes_histogram_parade"),
        picture("scopes_histogram_rgb"),
        "секции и наложение рисуют разную картинку"
    );
    assert_ne!(
        counters("scopes_histogram_luma"),
        counters("scopes_histogram_rgb"),
        "у режима Y одна плоскость яркости"
    );
    assert_ne!(
        counters("scopes_histogram_rgb"),
        counters("scopes_luma_waveform"),
        "гистограмма и LUMA WAVEFORM — разные счётчики"
    );

    // CIE: две шкалы — одни проходы, но разные числа правила, поэтому и счётчики, и картинки различаются; от гистограммы
    // и вектороскопа тоже (другая форма сетки и правило).
    assert_ne!(
        counters("scopes_cie_xy"),
        counters("scopes_cie_uv"),
        "шкалы xy и u′v′ кладут цвета в разные ячейки"
    );
    assert_ne!(
        picture("scopes_cie_xy"),
        picture("scopes_cie_uv"),
        "окна и локусы шкал разные"
    );
    assert_ne!(counters("scopes_cie_xy"), counters("scopes_histogram_rgb"));

    context.wait_idle().unwrap();
    assert_eq!(context.live_objects().differences_from(&baseline), Vec::new());
    assert_eq!(
        context.validation_report().errors,
        0,
        "validation: {:?}",
        context.validation_report()
    );
}
