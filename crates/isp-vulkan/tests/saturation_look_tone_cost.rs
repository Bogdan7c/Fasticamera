//! Цена стадии тона вида в видоискателе на ПК (RADV): время прохода `isp-end` при нейтральном виде и при всех четырёх
//! ручках тона на 0,5. Нейтраль — флаг в uniform (шейдер тон не считает), поэтому «нейтраль» обязана стоить как
//! видоискатель без стадии тона; «ручки на 0,5» — цена самой стадии.
//!
//! Замер идёт ЧЕРЕДУЯ A/B (нейтраль, тон, нейтраль, тон, …), чтобы дрейф частот GPU не приписывался одной стороне.
//! Порогов времени нет (на ПК с validation цифры завышены и шумят): тест проверяет, что оба варианта реально
//! отработали на продуктовом размере кадра и вариант с тоном действительно включает стадию, и печатает таблицу.
//! Запуск: `cargo test -p isp-vulkan --features test-support --test saturation_look_tone_cost -- --ignored --nocapture`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod look_support;
mod saturation_support;

use std::time::Duration;

use gpu_vulkan::saturation::{CancelToken, EndReason, SaturationBench, Throughput};
use isp_model::params::IspParams;
use isp_vulkan::saturation::{ViewfinderSaturation, ViewfinderSaturationConfig};
use look_contract::LookSettings;
use look_support::{controls, knob};
use saturation_support::viewfinder_support::{ParamsSpec, assert_no_validation_errors, gpu_context_or_skip};
use saturation_support::{bench_config, serialize_gpu_timing};

/// Сколько раз чередуются варианты.
const ROUNDS: usize = 5;
const WARMUP: Duration = Duration::from_millis(300);
const MEASURE: Duration = Duration::from_millis(1000);

/// Медиана прохода и стоимость кадра одного окна замера, миллисекунды.
struct WindowCost {
    pass_median_ms: f64,
    frame_cost_ms: f64,
}

fn measure(context: &gpu_vulkan::context::GpuContext, params: IspParams) -> WindowCost {
    let mut work = ViewfinderSaturation::new_synthetic(context, ViewfinderSaturationConfig::default(), params).unwrap();
    let slots = work.slots();
    let mut bench = SaturationBench::new(context, bench_config(slots, WARMUP, MEASURE)).unwrap();
    let report = bench.run(&mut work, &CancelToken::new()).unwrap();
    assert_eq!(report.end_reason, EndReason::Completed);
    let Throughput::Measured { frame_cost, .. } = report.throughput else {
        panic!("в окне нет кадров: {report:?}");
    };
    assert!(work.take_failure().is_none(), "кадры стенда с ошибкой");
    let pass = work
        .pass_times()
        .passes
        .into_iter()
        .next()
        .expect("проход isp-end измерен");
    WindowCost {
        pass_median_ms: pass.median.as_secs_f64() * 1e3,
        frame_cost_ms: frame_cost.as_secs_f64() * 1e3,
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[test]
#[ignore = "замер цены тона: долгий, печатает таблицу"]
fn tone_stage_cost_on_the_viewfinder_alternating_with_neutral() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let half = knob(0.5);
    let toned = spec.build_with_look(controls(LookSettings {
        contrast: half,
        midtones: half,
        shadows: half,
        highlights: half,
        ..LookSettings::NEUTRAL
    }));
    assert!(toned.look.tone.is_some(), "вариант «тон» включает стадию");
    let neutral = spec.build();
    assert!(neutral.look.tone.is_none(), "вариант «нейтраль» стадию пропускает");

    let (mut neutral_pass, mut toned_pass, mut neutral_frame, mut toned_frame) = (vec![], vec![], vec![], vec![]);
    for round in 0..ROUNDS {
        let n = measure(&context, neutral.clone());
        let t = measure(&context, toned.clone());
        println!(
            "раунд {round}: нейтраль проход {:.3} мс, кадр {:.3} мс | тон 0,5 проход {:.3} мс, кадр {:.3} мс",
            n.pass_median_ms, n.frame_cost_ms, t.pass_median_ms, t.frame_cost_ms
        );
        neutral_pass.push(n.pass_median_ms);
        toned_pass.push(t.pass_median_ms);
        neutral_frame.push(n.frame_cost_ms);
        toned_frame.push(t.frame_cost_ms);
    }
    println!(
        "МЕДИАНЫ ({}): нейтраль проход {:.3} мс / кадр {:.3} мс; тон 0,5 проход {:.3} мс / кадр {:.3} мс",
        context.capabilities().device_name,
        median(neutral_pass),
        median(neutral_frame),
        median(toned_pass),
        median(toned_frame)
    );
    assert_no_validation_errors(&context);
}
