//! Общие помощники тестов адаптеров стенда насыщения (сессия 16a, P4-isp): короткие конфиги стенда, замок
//! на время замера, конфиг ISP для маленьких кадров. Подготовка кадров и эталоны — из `viewfinder_support`.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "помощник тестов: каждый бинарь берёт свою часть; падение — способ сообщить о провале"
)]

#[path = "../viewfinder_support/mod.rs"]
pub mod viewfinder_support;

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::saturation::{CancelToken, SaturationBench, SaturationConfig, SaturationReport, SaturationWorkload};
use gpu_vulkan::swapchain::FramesInFlight;

/// Замер честен, только когда GPU занят одним тестом: тесты этого бинаря идут строго по очереди.
static GPU_TIMING_LOCK: Mutex<()> = Mutex::new(());

pub fn serialize_gpu_timing() -> MutexGuard<'static, ()> {
    GPU_TIMING_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Короткий конфиг стенда для тестов: прогрев 100 мс, окно 400 мс, шаг 100 мс.
pub fn quick_bench_config(frames_in_flight: usize) -> SaturationConfig {
    bench_config(frames_in_flight, Duration::from_millis(100), Duration::from_millis(400))
}

pub fn bench_config(frames_in_flight: usize, warmup: Duration, measure: Duration) -> SaturationConfig {
    SaturationConfig {
        frames_in_flight: FramesInFlight::new(frames_in_flight).unwrap(),
        warmup,
        measure,
        timeline_step: Duration::from_millis(100),
        fence_timeout: Duration::from_secs(30),
    }
}

/// Прогон работы стендом с коротким конфигом; без ошибок и с завершёнными кадрами.
pub fn run_quick(
    context: &GpuContext,
    workload: &mut dyn SaturationWorkload,
    frames_in_flight: usize,
) -> SaturationReport {
    let mut bench = SaturationBench::new(context, quick_bench_config(frames_in_flight)).unwrap();
    let report = bench.run(workload, &CancelToken::new()).unwrap();
    assert!(report.measured_frames > 0, "в окне нет кадров: {report:?}");
    report
}

use isp_model::image::DisplayGamut;
use isp_model::params::IspParams;
use isp_vulkan::saturation::ProductParamsRecipe;

/// Параметры `Quality` по калибровке реального кропа основной камеры (BGGR, RAW10-уровни 64/1023) — через рецепт
/// продукта (`ProductParamsRecipe::quality`), чтобы тесты шли ровно тем путём, которым пойдёт сценарий.
pub fn quality_params(gamut: DisplayGamut) -> IspParams {
    let capture = viewfinder_support::isp_support::real_capture::load_real_capture(
        &viewfinder_support::isp_support::assets_dir(),
        "real-main-lamp-gray-tile",
    );
    ProductParamsRecipe::quality(gamut)
        .resolve(&capture.sensor, capture.neutral)
        .unwrap()
}
