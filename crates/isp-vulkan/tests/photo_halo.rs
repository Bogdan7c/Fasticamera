//! Измерение радиуса зависимости демозаика на эталоне CPU (`isp-model`, `DemosaicMethod::Quality`): насколько
//! далеко от возмущённого отсчёта мозаики меняется результат. Это обоснование ореола полос
//! (`photo::slicing::DEMOSAIC_HALO`): ореол обязан быть не меньше измеренного радиуса, иначе полоса увидела бы
//! «неправильных» соседей и результат отличался бы от проявления целого кадра.
//!
//! Тест чисто CPU (без GPU): GPU-демозаик бит-в-бит равен CPU (допуск 0, `quality_demosaic_parity.rs`), поэтому
//! зависимость у них одна и та же.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod photo_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use isp_model::demosaic::{DemosaicMethod, demosaic};
use isp_model::image::{Mosaic, mosaic_stage::WhiteBalanced};
use isp_vulkan::photo::DEMOSAIC_HALO;
use photo_support::PATTERNS;
use quality_support::NoiseSource;

const SIDE: u32 = 72;

fn noisy_mosaic(pattern: isp_model::cfa::BayerPattern, seed: u32) -> Mosaic<WhiteBalanced> {
    let mut noise = NoiseSource::new(seed);
    let samples: Vec<f32> = (0..SIDE * SIDE).map(|_| 0.1 + 0.8 * noise.uniform()).collect();
    Mosaic::from_samples(PixelSize::new(SIDE, SIDE), pattern, samples, WhiteBalanced).unwrap()
}

/// Наибольшее расстояние (Чебышева) от возмущённого отсчёта `(qx, qy)` до пикселя, чей результат изменился.
fn observed_radius(mosaic: &Mosaic<WhiteBalanced>, baseline: &[[f32; 3]], qx: u32, qy: u32, delta: f32) -> u32 {
    let mut samples = mosaic.samples().to_vec();
    let index = (qy * SIDE + qx) as usize;
    samples[index] = (samples[index] + delta).clamp(0.0, 1.0);
    let perturbed = Mosaic::from_samples(mosaic.size(), mosaic.pattern(), samples, WhiteBalanced).unwrap();
    let result = demosaic(&perturbed, DemosaicMethod::Quality);
    let mut radius = 0;
    for (index, (got, want)) in result.pixels().iter().zip(baseline).enumerate() {
        if got.map(f32::to_bits) != want.map(f32::to_bits) {
            let (x, y) = (index as u32 % SIDE, index as u32 / SIDE);
            radius = radius.max(x.abs_diff(qx).max(y.abs_diff(qy)));
        }
    }
    radius
}

#[test]
fn demosaic_dependency_radius_is_within_the_halo_and_the_test_can_see_far_dependencies() {
    let mut widest = 0;
    for (pattern_index, pattern) in PATTERNS.into_iter().enumerate() {
        let mosaic = noisy_mosaic(pattern, 7 + pattern_index as u32);
        let baseline = demosaic(&mosaic, DemosaicMethod::Quality);
        // Возмущаем отсчёты в центре кадра (края и их зеркало не мешают: до границы больше радиуса), по всем
        // четырём позициям ячейки 2×2 и в обе стороны.
        for qy in [30, 31, 36, 37] {
            for qx in [29, 30, 35, 38] {
                for delta in [0.3_f32, -0.7] {
                    let radius = observed_radius(&mosaic, baseline.pixels(), qx, qy, delta);
                    widest = widest.max(radius);
                    assert!(
                        radius < DEMOSAIC_HALO,
                        "{pattern:?} возмущение ({qx},{qy}) {delta}: результат изменился на расстоянии {radius} — \
                         ореол {DEMOSAIC_HALO} недостаточен"
                    );
                }
            }
        }
    }
    println!("наибольший измеренный радиус зависимости демозаика: {widest} пикселей (ореол {DEMOSAIC_HALO})");
    // Тест «видит» дальние зависимости: если бы шумящая мозаика меняла только ближайшие пиксели, проверка
    // «радиус < ореол» ничего не доказывала. Измерено 8 (строгая оценка по таблице проходов — 11: шаги проходов
    // идут вдоль осей и чередуют чётность узлов, поэтому не все крайние пути совмещаются); ореол 4 тест бы
    // отверг, ореол 8 — тоже (нужно строго больше радиуса).
    assert!(
        widest >= 8,
        "измеренный радиус {widest} слишком мал: тест не отличает ореол 12 от ореола 4"
    );
}
