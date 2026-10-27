//! Качество демозаика Menon 2007 против билинейного базового уровня на синтетике.
//!
//! Билинейный демозаик реализован здесь же, только для сравнения: он даёт «нижнюю планку»,
//! которую Menon обязан заметно побеждать по точности (PSNR) и по ложным цветам.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в интеграционных тестах падение и есть способ сообщить о провале"
)]

use camera_contract::geometry::PixelSize;
use isp_model::cfa::BayerPattern;
use isp_model::demosaic::{DemosaicMethod, demosaic};
use isp_model::image::{LinearCameraRgb, WhiteBalancedMosaic, space};
use isp_model::test_patterns::{color_bars, mosaic_from_rgb, slanted_edge, zone_plate};

/// Отступ от края кадра при оценке метрик: у границы все алгоритмы деградируют, а паритет границ
/// проверяет отдельный тест.
const METRIC_MARGIN: u32 = 4;

/// Билинейный демозаик: каждый недостающий канал — среднее отсчётов этого канала в окне 3×3
/// (на краю окно усечено). Для Bayer это классическая билинейная интерполяция.
fn bilinear(mosaic: &WhiteBalancedMosaic) -> LinearCameraRgb {
    let size = mosaic.size();
    let pattern = mosaic.pattern();
    let mut pixels = Vec::new();
    for y in 0..size.height {
        for x in 0..size.width {
            let mut sums = [0.0_f32; 3];
            let mut counts = [0u32; 3];
            for ny in y.saturating_sub(1)..=(y + 1).min(size.height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(size.width - 1) {
                    let channel = pattern.channel_at(nx, ny).index();
                    sums[channel] += mosaic.sample(nx, ny);
                    counts[channel] += 1;
                }
            }
            pixels.push([0, 1, 2].map(|c| sums[c] / counts[c] as f32));
        }
    }
    LinearCameraRgb::from_pixels(size, pixels, space::CameraRgb).unwrap()
}

fn interior(size: PixelSize) -> impl Iterator<Item = (u32, u32)> {
    (METRIC_MARGIN..size.height - METRIC_MARGIN)
        .flat_map(move |y| (METRIC_MARGIN..size.width - METRIC_MARGIN).map(move |x| (x, y)))
}

/// PSNR (пик = 1) по внутренней области всех трёх каналов.
fn psnr(result: &LinearCameraRgb, original: &LinearCameraRgb) -> f64 {
    let (mut sum, mut count) = (0.0_f64, 0u64);
    for (x, y) in interior(original.size()) {
        for channel in 0..3 {
            let difference = f64::from(result.pixel(x, y)[channel] - original.pixel(x, y)[channel]);
            sum += difference * difference;
            count += 1;
        }
    }
    -10.0 * (sum / count as f64).log10()
}

/// Ложный цвет на нейтральной цели: среднее |R−G| + |B−G| по внутренней области.
fn mean_chroma(result: &LinearCameraRgb) -> f64 {
    let (mut sum, mut count) = (0.0_f64, 0u64);
    for (x, y) in interior(result.size()) {
        let [red, green, blue] = result.pixel(x, y);
        sum += f64::from((red - green).abs() + (blue - green).abs());
        count += 1;
    }
    sum / count as f64
}

/// Максимальный ложный цвет (худший пиксель) — «зиппер» вдоль края.
fn max_chroma(result: &LinearCameraRgb) -> f32 {
    interior(result.size())
        .map(|(x, y)| {
            let [red, green, blue] = result.pixel(x, y);
            (red - green).abs() + (blue - green).abs()
        })
        .fold(0.0, f32::max)
}

fn both(original: &LinearCameraRgb, pattern: BayerPattern) -> (LinearCameraRgb, LinearCameraRgb) {
    let mosaic = mosaic_from_rgb(original, pattern).unwrap();
    (demosaic(&mosaic, DemosaicMethod::Quality), bilinear(&mosaic))
}

const ALL_PATTERNS: [BayerPattern; 4] = [
    BayerPattern::RGGB,
    BayerPattern::GRBG,
    BayerPattern::GBRG,
    BayerPattern::BGGR,
];

#[test]
fn zone_plate_is_reconstructed_better_and_with_less_false_colour_than_bilinear() {
    let original = zone_plate(PixelSize::new(128, 128), 0.5).unwrap();
    for pattern in ALL_PATTERNS {
        let (menon, baseline) = both(&original, pattern);
        let (menon_psnr, bilinear_psnr) = (psnr(&menon, &original), psnr(&baseline, &original));
        let (menon_chroma, bilinear_chroma) = (mean_chroma(&menon), mean_chroma(&baseline));
        println!(
            "{pattern:?}: PSNR menon {menon_psnr:.2} bilinear {bilinear_psnr:.2}; chroma menon {menon_chroma:.4} bilinear {bilinear_chroma:.4}"
        );
        assert!(menon_psnr > bilinear_psnr + MIN_PSNR_GAIN_DB, "{pattern:?}");
        assert!(menon_psnr > MIN_ZONE_PLATE_PSNR_DB, "{pattern:?}");
        assert!(
            menon_chroma < bilinear_chroma * f64::from(MAX_CHROMA_RATIO),
            "{pattern:?}"
        );
    }
}

#[test]
fn slanted_edge_has_no_zipper_artifacts() {
    let original = slanted_edge(PixelSize::new(64, 64), 5.0, 0.1, 0.9).unwrap();
    for pattern in ALL_PATTERNS {
        let (menon, baseline) = both(&original, pattern);
        let (menon_max, bilinear_max) = (max_chroma(&menon), max_chroma(&baseline));
        println!("{pattern:?}: edge max chroma menon {menon_max:.4} bilinear {bilinear_max:.4}");
        assert!(menon_max < MAX_EDGE_CHROMA, "{pattern:?}");
        assert!(menon_max < bilinear_max * MAX_CHROMA_RATIO, "{pattern:?}");
    }
}

/// Вертикальные серые полосы периода 4 (2 светлых, 2 тёмных) или горизонтальные при `vertical == false`.
fn stripes(size: PixelSize, direction: StripeDirection) -> LinearCameraRgb {
    let pixels = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let along = match direction {
                StripeDirection::Vertical => x,
                StripeDirection::Horizontal => y,
            };
            let level = if (along / 2) % 2 == 0 {
                STRIPE_BRIGHT
            } else {
                STRIPE_DARK
            };
            [level; 3]
        })
        .collect();
    LinearCameraRgb::from_pixels(size, pixels, space::CameraRgb).unwrap()
}

#[derive(Clone, Copy)]
enum StripeDirection {
    Vertical,
    Horizontal,
}

/// Ломается, если решение H/V инвертировано: на полосах единственное верное направление интерполяции
/// зелёного — вдоль полос, там ошибка нулевая; поперёк полос — большая.
#[test]
fn stripes_are_reconstructed_exactly_along_the_stripe_direction() {
    for direction in [StripeDirection::Vertical, StripeDirection::Horizontal] {
        let original = stripes(PixelSize::new(32, 32), direction);
        for pattern in ALL_PATTERNS {
            let (menon, baseline) = both(&original, pattern);
            let worst_green_error = |image: &LinearCameraRgb| {
                interior(image.size())
                    .map(|(x, y)| (image.pixel(x, y)[1] - original.pixel(x, y)[1]).abs())
                    .fold(0.0_f32, f32::max)
            };
            let (menon_error, bilinear_error) = (worst_green_error(&menon), worst_green_error(&baseline));
            assert!(menon_error < 1e-4, "{pattern:?}: ошибка зелёного Menon {menon_error}");
            assert!(
                bilinear_error > 0.1,
                "{pattern:?}: билинейный не ошибается? {bilinear_error}"
            );
            assert!(psnr(&menon, &original) > 80.0, "{pattern:?}");
        }
    }
}

#[test]
fn color_bars_keep_saturated_colours_inside_the_bars() {
    // Внутри полосы (в стороне от границ) цвет обязан восстановиться почти точно.
    let original = color_bars(PixelSize::new(56, 16), 0.8).unwrap();
    for pattern in ALL_PATTERNS {
        let menon = demosaic(&mosaic_from_rgb(&original, pattern).unwrap(), DemosaicMethod::Quality);
        // Полосы по 8 пикселей: берём центры (x % 8 == 3 или 4).
        for y in METRIC_MARGIN..16 - METRIC_MARGIN {
            for x in (0..56).filter(|x| matches!(x % 8, 3 | 4)) {
                let (got, want) = (menon.pixel(x, y), original.pixel(x, y));
                assert!(
                    (0..3).all(|c| (got[c] - want[c]).abs() < 1e-4),
                    "{pattern:?} ({x}, {y}): {got:?} против {want:?}"
                );
            }
        }
    }
}

const STRIPE_BRIGHT: f32 = 0.8;
const STRIPE_DARK: f32 = 0.2;
// Пороги выбраны по измерениям (`cargo test --test demosaic_quality -- --nocapture`) с запасом ~2×.
// Зонная пластинка 128×128, до 0,5 цикл/пиксель в углу, одинакова для всех 4 раскладок:
//   PSNR Menon 35,08 дБ, билинейный 12,20 дБ (выигрыш ≈ 22,9 дБ);
//   средний ложный цвет Menon 0,0304, билинейный 0,2340 (отношение ≈ 0,13).
// Наклонный край (5°, 0,1…0,9, 64×64): худший ложный цвет Menon 0,0152…0,0157, билинейный 0,377…0,381.
/// Минимальный выигрыш Menon над билинейным по PSNR, дБ.
const MIN_PSNR_GAIN_DB: f64 = 12.0;
/// Абсолютная нижняя граница PSNR на зонной пластинке, дБ.
const MIN_ZONE_PLATE_PSNR_DB: f64 = 30.0;
/// Menon должен давать не более этой доли ложного цвета билинейного (измерено ≈ 0,13 и ≈ 0,04).
const MAX_CHROMA_RATIO: f32 = 0.3;
/// Худший ложный цвет вдоль края: «зиппер» дал бы десятые доли, измерено ≈ 0,016.
const MAX_EDGE_CHROMA: f32 = 0.05;
