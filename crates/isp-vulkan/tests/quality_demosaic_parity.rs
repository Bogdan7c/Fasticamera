//! Паритет GPU-демозаика Menon 2007 (`QualityDemosaic`) с эталоном CPU `isp-model`
//! (`DemosaicMethod::Quality`) и с независимым эталоном colour-demosaicing 0.2.7 (фикстуры `demosaic-menon-*`).
//!
//! Вход — одна и та же сбалансированная мозаика f32 (загружается прямо в плоскость мозаики), поэтому
//! сравнивается только демозаик: все 4 раскладки CFA, все пиксели, включая края кадра.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::LocalSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::{DemosaicMethod, demosaic};
use isp_model::image::{LinearCameraRgb, space};
use isp_model::test_patterns::{color_bars, mosaic_from_rgb, slanted_edge, zone_plate};
use isp_vulkan::quality::{CfaCode, QualityDemosaic};
use quality_support::{
    assert_no_validation_errors, extent, gpu_context_or_skip, max_rgb_difference, read_rgb, upload_plane,
};

/// Допуск GPU против CPU-эталона той же модели (оба f32): НОЛЬ — совпадение бит в бит.
///
/// Почему это достижимо: шейдеры повторяют порядок сложений эталона (`precise` запрещает FMA и
/// перестановки), сложение/вычитание/умножение в Vulkan округляются правильно (таблица точности SPIR-V),
/// умножения на 0,5/0,25 точны, а единственное деление (на 3 в шаге уточнения) идёт через
/// `divide_correctly_rounded` (`quality_common.glsl`): OpFDiv сам даёт до 2,5 ULP, и даже 1 ULP
/// переворачивает решение H/V у почти равных оценок (замерено: 1e-3 на зонной пластинке).
/// Замер на RADV — 0 на всех случаях. Ненулевая разница на другом драйвере означает нарушение
/// правила округления (например, не RTE), а не «шум».
const CPU_PARITY_TOLERANCE: f32 = 0.0;

/// Допуск против colour-demosaicing (эталон считает в f64, фикстура хранит f32): тот же, что у теста
/// паритета `isp-model` (1e-6; измерено ≤ 1,8e-7 ≈ 1,5 ULP) — GPU не дальше от него, чем CPU-модель.
const REFERENCE_PARITY_TOLERANCE: f32 = 1e-6;

const PATTERNS: [(BayerPattern, &str); 4] = [
    (BayerPattern::RGGB, "rggb"),
    (BayerPattern::GRBG, "grbg"),
    (BayerPattern::GBRG, "gbrg"),
    (BayerPattern::BGGR, "bggr"),
];

/// Прогоняет мозаику сцены через GPU и CPU; возвращает (GPU, CPU) пиксели.
fn demosaic_both(
    context: &GpuContext,
    scene: &LinearCameraRgb,
    pattern: BayerPattern,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mosaic = mosaic_from_rgb(scene, pattern).unwrap();
    let cpu = demosaic(&mosaic, DemosaicMethod::Quality);
    let size = mosaic.size();
    let gpu_demosaic = QualityDemosaic::new(
        context,
        extent(size.width, size.height),
        LocalSize::new(16, 8).unwrap(),
        1,
    )
    .unwrap();
    upload_plane(gpu_demosaic.planes().mosaic(), mosaic.samples());
    run_one_shot_blocking(context, |command_buffer| {
        gpu_demosaic.record_all(context, command_buffer, 0, CfaCode::from_pattern(pattern))
    })
    .unwrap();
    let gpu = read_rgb(gpu_demosaic.planes().linear_rgb());
    (gpu, cpu.pixels().to_vec())
}

fn assert_matches_cpu(context: &GpuContext, name: &str, scene: &LinearCameraRgb) {
    for (pattern, pattern_name) in PATTERNS {
        let (gpu, cpu) = demosaic_both(context, scene, pattern);
        let (difference, (x, y)) = max_rgb_difference(&gpu, &cpu, scene.size().width);
        println!("{name}/{pattern_name}: GPU vs isp-model max |Δ| = {difference:e} в ({x}, {y})");
        assert!(
            difference <= CPU_PARITY_TOLERANCE,
            "{name}/{pattern_name}: расхождение {difference:e} в ({x}, {y})"
        );
    }
}

#[test]
fn gpu_menon_matches_cpu_model_and_colour_demosaicing_on_fixtures() {
    let Some(context) = gpu_context_or_skip() else { return };
    let scene_asset = isp_support::load_asset("demosaic-scene");
    let scene = LinearCameraRgb::from_pixels(
        PixelSize::new(scene_asset.width, scene_asset.height),
        scene_asset.rgb_pixels(),
        space::CameraRgb,
    )
    .unwrap();
    for (pattern, pattern_name) in PATTERNS {
        let (gpu, cpu) = demosaic_both(&context, &scene, pattern);
        let width = scene.size().width;
        let (cpu_difference, cpu_at) = max_rgb_difference(&gpu, &cpu, width);
        let reference = isp_support::load_asset(&format!("demosaic-menon-{pattern_name}")).rgb_pixels();
        let (reference_difference, reference_at) = max_rgb_difference(&gpu, &reference, width);
        println!(
            "fixture/{pattern_name}: GPU vs isp-model {cpu_difference:e} в {cpu_at:?}, \
             GPU vs colour-demosaicing {reference_difference:e} в {reference_at:?}"
        );
        assert!(
            cpu_difference <= CPU_PARITY_TOLERANCE,
            "{pattern_name}: {cpu_difference:e} в {cpu_at:?}"
        );
        assert!(
            reference_difference <= REFERENCE_PARITY_TOLERANCE,
            "{pattern_name}: {reference_difference:e} в {reference_at:?}"
        );
        // Демозаик реально восстанавливает сцену, а не копирует мозаику: в R-узлах появились G и B.
        let scene_error = gpu
            .iter()
            .zip(scene.pixels())
            .flat_map(|(got, want)| (0..3).map(move |channel| (got[channel] - want[channel]).abs()))
            .fold(0.0_f32, f32::max);
        assert!(
            scene_error < 1.0,
            "{pattern_name}: GPU-результат не похож на сцену ({scene_error})"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn gpu_menon_matches_cpu_model_on_synthetic_hard_cases() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Зонная пластинка до Найквиста (муар, решение H/V по всему кадру), наклонные края (решения у края),
    // цветные полосы (резкие цветовые скачки), нечётные «по 2» размеры, ширина 2 (зеркало короче ядра).
    let cases: Vec<(&str, LinearCameraRgb)> = vec![
        ("zone_plate_96x64", zone_plate(PixelSize::new(96, 64), 0.5).unwrap()),
        (
            "edge_5deg_62x46",
            slanted_edge(PixelSize::new(62, 46), 5.0, 0.05, 0.9).unwrap(),
        ),
        (
            "edge_40deg_64x64",
            slanted_edge(PixelSize::new(64, 64), 40.0, 0.1, 0.8).unwrap(),
        ),
        ("color_bars_70x30", color_bars(PixelSize::new(70, 30), 0.85).unwrap()),
        ("zone_plate_2x6", zone_plate(PixelSize::new(2, 6), 0.5).unwrap()),
    ];
    for (name, scene) in &cases {
        assert_matches_cpu(&context, name, scene);
    }
    assert_no_validation_errors(&context);
}
