//! Паритет демозаика Menon 2007 с colour-demosaicing 0.2.7 (фикстуры `demosaic-*`).
//!
//! Сцена из фикстуры превращается в мозаику каждой из 4 раскладок, Rust-демозаик сравнивается
//! с f64-результатом эталона по всем пикселям, включая границу кадра.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в интеграционных тестах падение и есть способ сообщить о провале"
)]

mod support;

use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::{DemosaicMethod, demosaic};
use isp_model::image::{LinearCameraRgb, space};
use isp_model::test_patterns::mosaic_from_rgb;

/// Допуск паритета. Эталон считает в f64, Rust — в f32 (машинный эпсилон ≈ 1,2e-7 при значениях ~1).
/// Измеренное максимальное расхождение — 1,19e-7 (один ULP f32) для всех раскладок; допуск 1e-6 даёт
/// запас ~8× на другие платформы, но всё ещё ловит любую ошибку формулы (неверный вес или
/// направление H/V дают расхождения ~1e-2).
const PARITY_TOLERANCE: f32 = 1e-6;

fn scene() -> LinearCameraRgb {
    let asset = support::load_asset("demosaic-scene");
    LinearCameraRgb::from_pixels(
        PixelSize::new(asset.width, asset.height),
        asset.rgb_pixels(),
        space::CameraRgb,
    )
    .expect("сцена фикстуры валидна")
}

/// Максимум |Rust − эталон| и пиксель, где он достигнут.
fn max_difference(pattern: BayerPattern, fixture_name: &str) -> (f32, (u32, u32)) {
    let scene = scene();
    let mosaic = mosaic_from_rgb(&scene, pattern).expect("мозаика сцены");
    let rust = demosaic(&mosaic, DemosaicMethod::Quality);
    let reference = support::load_asset(fixture_name);
    assert_eq!(
        (reference.width, reference.height),
        (rust.size().width, rust.size().height)
    );
    let expected = reference.rgb_pixels();
    let mut worst = (0.0_f32, (0, 0));
    for y in 0..rust.size().height {
        for x in 0..rust.size().width {
            let want = expected[(y * reference.width + x) as usize];
            let got = rust.pixel(x, y);
            for channel in 0..3 {
                let difference = (got[channel] - want[channel]).abs();
                if difference > worst.0 {
                    worst = (difference, (x, y));
                }
            }
        }
    }
    worst
}

fn assert_parity(pattern: BayerPattern, fixture_name: &str) {
    let (difference, (x, y)) = max_difference(pattern, fixture_name);
    println!("{fixture_name}: max abs diff {difference:e} at ({x}, {y})");
    assert!(
        difference <= PARITY_TOLERANCE,
        "{fixture_name}: расхождение {difference:e} в пикселе ({x}, {y})"
    );
}

#[test]
fn menon_matches_reference_for_rggb() {
    assert_parity(BayerPattern::RGGB, "demosaic-menon-rggb");
}

#[test]
fn menon_matches_reference_for_grbg() {
    assert_parity(BayerPattern::GRBG, "demosaic-menon-grbg");
}

#[test]
fn menon_matches_reference_for_gbrg() {
    assert_parity(BayerPattern::GBRG, "demosaic-menon-gbrg");
}

#[test]
fn menon_matches_reference_for_bggr() {
    assert_parity(BayerPattern::BGGR, "demosaic-menon-bggr");
}

/// Сцена в фикстуре не тривиальна: края и решётка реально меняют результат против сырой мозаики.
/// Без этой проверки паритет «совпал бы» и на пустой фикстуре.
#[test]
fn reference_fixture_is_non_trivial() {
    let reference = support::load_asset("demosaic-menon-rggb");
    let scene = scene();
    let largest_reconstruction_error = reference
        .rgb_pixels()
        .iter()
        .zip(scene.pixels())
        .flat_map(|(got, want)| (0..3).map(move |c| (got[c] - want[c]).abs()))
        .fold(0.0_f32, f32::max);
    assert!(
        largest_reconstruction_error > 0.05,
        "сцена слишком простая для проверки алгоритма"
    );
}
