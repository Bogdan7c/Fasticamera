//! Столбцы следа RGB waveform и parade на GPU (корень тёмных полос 14o, решение 322).
//!
//! В разделе калибровки окно, которое меряет прибор, ≈ 917 px вдоль взгляда; при прежней фиксированной сетке
//! 512 столбцов и шаге 2 54 столбца не получали ни одного отсчёта — тёмные полосы. Теперь столбец сетки — номер
//! отсчёта вдоль взгляда. Тест на настоящем GPU: окно ВДВОЕ уже прежней сетки (459 px при шаге 2 — 230 отсчётов),
//! все четыре положения телефона:
//! * в КАЖДОМ столбце счётчиков GPU ровно столько попаданий, сколько отсчётов поперёк взгляда (пустых нет);
//! * картинка прибора GPU совпадает с эталоном `scope-model` в допуске, след нарисован.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame};
use scope_contract::ScopeScan;
use scope_model::{SampleStride, ScopeModelSettings, render};
use scope_support::{
    Scene, Xorshift, assert_no_validation_errors, gpu_context_or_skip, picture_size, planned_samples_for,
    rule_cases_for_scan, run_scope_frame, upload_scene,
};

/// Допуск сравнения картинки (тот же, что у `scope_picture.rs`).
const PICTURE_TOLERANCE: f32 = 2.0e-3;

/// Длина окна вдоль взгляда: вдвое меньше, чем прежняя сетка требовала при шаге 2 (512 · 2 = 1024).
const EYE_LENGTH: u32 = 459;
/// Поперёк взгляда.
const ACROSS_EYE: u32 = 200;

/// Случайная картинка `width × height`: коды разбросаны по уровням, ячейки следа заполнены редко.
fn random_scene(name: &'static str, width: u32, height: u32) -> Scene {
    let mut random = Xorshift(0x0BAD_5EED);
    let codes = (0..width * height)
        .map(|_| [0, 1, 2].map(|_| (random.next() % 1024) as u16))
        .collect();
    Scene {
        name,
        width,
        height,
        codes,
    }
}

fn settings() -> ScopeModelSettings {
    ScopeModelSettings {
        trace_levels: 128,
        vectorscope_size: 64,
        sample_stride: SampleStride::EVERY_SECOND_PIXEL,
        ..ScopeModelSettings::default()
    }
}

#[test]
fn every_column_gets_the_same_samples_and_the_picture_matches_the_reference_on_a_narrow_window_for_every_scan() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = settings();
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    // Картинка шире окна (увеличение, как 917 → 1011 в разделе).
    let size = picture_size(506, 96);
    for (scan, scene) in [
        (
            ScopeScan::Upright,
            random_scene("narrow-459x200", EYE_LENGTH, ACROSS_EYE),
        ),
        (
            ScopeScan::UpsideDown,
            random_scene("narrow-459x200", EYE_LENGTH, ACROSS_EYE),
        ),
        (
            ScopeScan::RotatedClockwise,
            random_scene("tall-200x459", ACROSS_EYE, EYE_LENGTH),
        ),
        (
            ScopeScan::RotatedCounterClockwise,
            random_scene("tall-200x459", ACROSS_EYE, EYE_LENGTH),
        ),
    ] {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (4, 2));
        let shown = input.shown_codes(HorizontalMirror::Off);
        for (rule_name, rule) in rule_cases_for_scan(&settings, scan)
            .into_iter()
            .filter(|(name, _)| matches!(*name, "waveform" | "luma" | "parade"))
        {
            // План прибора: шаг вдоль взгляда — из настроек, поперёк — свой у следов (решение 329); при повороте
            // оси поверхности меняются местами.
            let samples = planned_samples_for(&shown, &settings, &rule);
            let (along, across) = (samples.along_eye(scan), samples.across_eye(scan));
            // Предпосылка: при прежней сетке столбцов было бы больше, чем отсчётов вдоль взгляда (были бы пустые).
            assert!(
                along < 512,
                "{scan:?}: окно должно быть уже прежней сетки, отсчётов {along}"
            );
            let frame = ScopeFrame {
                input: input.input(HorizontalMirror::Off),
                rule: &rule,
                settings: &settings,
                picture_size: size,
                frame_slot: 0,
            };
            run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let counters = scopes.read_back_counters_blocking().unwrap();
            assert_eq!(
                counters.shape().columns(),
                along,
                "{rule_name} / {scan:?}: столбец на отсчёт"
            );
            for plane in 0..counters.shape().planes() {
                for column in 0..along {
                    let hits: u32 = (0..counters.shape().rows())
                        .map(|row| counters.count(plane, row, column).unwrap())
                        .sum();
                    assert_eq!(
                        hits, across,
                        "{rule_name} / {scan:?}: плоскость {plane}, столбец {column}"
                    );
                }
            }
            let gpu = scopes.read_back_picture_blocking().unwrap();
            let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
            assert_eq!(gpu.texels.len(), cpu.pixels().len());
            let premultiplied =
                |pixel: [f32; 4]| [pixel[0] * pixel[3], pixel[1] * pixel[3], pixel[2] * pixel[3], pixel[3]];
            let worst = gpu
                .texels
                .iter()
                .zip(cpu.pixels())
                .flat_map(|(gpu_pixel, cpu_pixel)| {
                    let (gpu_value, cpu_value) = (premultiplied(*gpu_pixel), premultiplied(*cpu_pixel));
                    (0..4).map(move |channel| (gpu_value[channel] - cpu_value[channel]).abs())
                })
                .fold(0.0_f32, f32::max);
            assert!(
                worst <= PICTURE_TOLERANCE,
                "{rule_name} / {scan:?} / {}: расхождение {worst}",
                scene.name
            );
            assert!(
                gpu.texels.iter().filter(|pixel| pixel[3] > 0.1).count() > 1000,
                "{rule_name} / {scan:?}: след не нарисован"
            );
        }
    }
    assert_no_validation_errors(&context);
}
