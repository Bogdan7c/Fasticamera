//! Замер цены приборов на ПК (RADV, полная validation): GPU-время накопления и показа кадра 1920×1080 при
//! настройках по умолчанию (сетки 256, шаг 2) и счётчики, совпадающие с эталоном на том же кадре.
//!
//! Цифры RADV — не цифры телефона: они нужны как порядок величины и защита от регрессий; замер на Adreno — 14b.
//! Тест ничего не проверяет по времени жёстко (разные машины), но требует, чтобы метки были получены, а в
//! каждом кадре время накопления и показа было положительным.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use ash::vk;
use gpu_vulkan::gpu_timing::TimestampCollect;
use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame};
use scope_model::{ScopeModelSettings, ScopeRule};
use scope_support::{
    Xorshift, assert_no_validation_errors, cpu_counters, display_value, gpu_context_or_skip, picture_size, rule_cases,
    run_scope_frame, upload_input,
};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const MEASURED_FRAMES: usize = 8;

/// Кадр «как с камеры»: плавный цветной градиент с лёгким шумом.
fn natural_like_texels() -> Vec<[f32; 4]> {
    let mut noise = Xorshift(0xC0FF_EE11);
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .map(|(x, y)| {
            let base = [
                x * 1023 / (WIDTH - 1),
                y * 1023 / (HEIGHT - 1),
                (x + y) * 1023 / (WIDTH + HEIGHT - 2),
            ];
            let jitter = |value: u32, random: u32| (value + random % 24).saturating_sub(12).min(1023) as u16;
            [
                display_value(jitter(base[0], noise.next())),
                display_value(jitter(base[1], noise.next())),
                display_value(jitter(base[2], noise.next())),
                1.0,
            ]
        })
        .collect()
}

#[test]
fn measure_gpu_time_of_each_scope_on_a_1080p_frame_and_keep_bit_exact_counters() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings::default();
    let extent = vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    };
    let input = upload_input(
        &context,
        extent,
        extent,
        StorageFormatPolicy::PreferTenBitUnorm,
        &natural_like_texels(),
    );
    let shown = input.shown_codes(HorizontalMirror::Off);
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    for (name, rule) in rule_cases(&settings) {
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: picture_size(540, 360),
            frame_slot: 0,
        };
        let mut accumulate_ns = Vec::new();
        let mut render_ns = Vec::new();
        for frame_index in 0..=MEASURED_FRAMES {
            let (_, report) = run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let TimestampCollect::Ready(spans) = report.gpu_time else {
                panic!("{name}: метки времени не получены: {:?}", report.gpu_time);
            };
            // Первый кадр — прогрев (кэши шейдера, раскладки).
            if frame_index > 0 {
                let span_of = |label: &str| {
                    spans
                        .spans
                        .iter()
                        .find(|span| span.to == label)
                        .map(|span| span.duration_ns)
                        .unwrap()
                };
                accumulate_ns.push(span_of("accumulate-end"));
                render_ns.push(span_of("render-end"));
            }
        }
        accumulate_ns.sort_unstable();
        render_ns.sort_unstable();
        let median = |values: &[u64]| values[values.len() / 2] as f64 / 1.0e6;
        eprintln!(
            "RADV {name}: накопление {:.3} мс, показ {:.3} мс (медиана из {MEASURED_FRAMES}, 1080p, шаг 2, сетки 256)",
            median(&accumulate_ns),
            median(&render_ns)
        );
        assert!(accumulate_ns.iter().all(|&ns| ns > 0), "{name}: накопление измерено");
        assert!(render_ns.iter().all(|&ns| ns > 0), "{name}: показ измерен");
        // Счётчики последнего кадра совпадают с эталоном на этом же полном кадре.
        assert_eq!(
            scopes.read_back_counters_blocking().unwrap(),
            cpu_counters(&shown, &settings, &rule as &dyn ScopeRule),
            "{name}: счётчики 1080p"
        );
    }
    assert_no_validation_errors(&context);
}
