//! Печатающий стенд «до» (сессия 16a, P4-isp): все четыре работы на продуктовых размерах под стендом
//! насыщения. Порогов нет — таблица для сравнения «до/после» оптимизаций (RADV на ПК, Adreno 740 на телефоне).
//! Запуск: `cargo test -p isp-vulkan --features test-support --test saturation_bench -- --nocapture`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use std::time::Duration;

use gpu_vulkan::saturation::PassTimeSummary;
use gpu_vulkan::saturation::{
    CancelToken, EndReason, SaturationBench, SaturationReport, SaturationWorkload, Throughput,
};
use isp_model::image::DisplayGamut;
use isp_vulkan::saturation::{
    HeadroomSaturation, HeadroomSaturationConfig, ScopesSaturation, ScopesSaturationConfig, StillSaturation,
    StillSaturationConfig, ViewfinderSaturation, ViewfinderSaturationConfig,
};
use saturation_support::viewfinder_support::{ParamsSpec, assert_no_validation_errors, gpu_context_or_skip};
use saturation_support::{bench_config, quality_params, serialize_gpu_timing};

const WARMUP: Duration = Duration::from_millis(200);
const MEASURE: Duration = Duration::from_millis(700);

fn run(context: &gpu_vulkan::context::GpuContext, work: &mut dyn SaturationWorkload, slots: usize) -> SaturationReport {
    let mut bench = SaturationBench::new(context, bench_config(slots, WARMUP, MEASURE)).unwrap();
    bench.run(work, &CancelToken::new()).unwrap()
}

/// Строка таблицы: работа → к/с → мс/кадр → медианы проходов.
fn print_row(name: &str, report: &SaturationReport, times: &PassTimeSummary) {
    assert_eq!(report.end_reason, EndReason::Completed);
    let Throughput::Measured {
        frames_per_second,
        frame_cost,
    } = report.throughput
    else {
        panic!("{name}: в окне нет кадров: {report:?}");
    };
    let passes: Vec<String> = times
        .passes
        .iter()
        .map(|pass| format!("{} {:.3}", pass.label, pass.median.as_secs_f64() * 1e3))
        .collect();
    println!(
        "| {name:<12} | {frames_per_second:>8.1} | {:>8.3} | {} |",
        frame_cost.as_secs_f64() * 1e3,
        if passes.is_empty() {
            "(без замера по проходам)".to_owned()
        } else {
            passes.join("; ")
        }
    );
}

#[test]
fn saturation_bench_prints() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    println!(
        "\nСтенд насыщения, {}, прогрев {WARMUP:?}, окно {MEASURE:?}, 2 слота (validation FULL: цифры завышены)",
        context.capabilities().device_name
    );
    println!("| работа       |      к/с |  мс/кадр | медианы проходов, мс (GPU) |");
    println!("|--------------|----------|----------|-----------------------------|");

    let mut viewfinder = ViewfinderSaturation::new_synthetic(
        &context,
        ViewfinderSaturationConfig::default(),
        ParamsSpec::default().build(),
    )
    .unwrap();
    let slots = viewfinder.slots();
    let report = run(&context, &mut viewfinder, slots);
    print_row("viewfinder", &report, &viewfinder.pass_times());
    assert!(viewfinder.take_failure().is_none());
    drop(viewfinder);

    for (name, kind) in [
        ("scopes-wave", scope_contract::ScopeKind::Waveform),
        ("scopes-vec", scope_contract::ScopeKind::Vectorscope),
    ] {
        let config = ScopesSaturationConfig {
            kind,
            ..ScopesSaturationConfig::default()
        };
        let mut scopes = ScopesSaturation::new_synthetic(&context, config, ParamsSpec::default().build()).unwrap();
        let slots = scopes.slots();
        let report = run(&context, &mut scopes, slots);
        print_row(name, &report, &scopes.pass_times());
        assert!(scopes.take_failure().is_none());
    }

    let mut headroom = HeadroomSaturation::new_synthetic(
        &context,
        HeadroomSaturationConfig::default(),
        quality_params(DisplayGamut::Srgb),
    )
    .unwrap();
    let slots = headroom.slots();
    let report = run(&context, &mut headroom, slots);
    print_row("headroom-v4", &report, &headroom.pass_times());
    assert!(headroom.take_failure().is_none());
    drop(headroom);

    let mut still = StillSaturation::new_synthetic(
        &context,
        StillSaturationConfig::default(),
        quality_params(DisplayGamut::Srgb),
    )
    .unwrap();
    println!("(снимок: шагов записи на кадр = {})", still.steps_per_still());
    let slots = still.slots();
    let report = run(&context, &mut still, slots);
    print_row("still", &report, &still.pass_times());
    assert!(still.take_failure().is_none());
    still.finish_pending_blocking().unwrap();
    drop(still);
    context.wait_idle().unwrap();
    assert_no_validation_errors(&context);
}
