//! Реальные кропы RAW (C7 «лампа», основная камера) через весь эталон против внешних эталонов.
//!
//! Фикстуры и их эталоны генерирует `tools/isp-reference/make_real_crops.py` (команда — в
//! `test-assets/isp-model/README.md`).
//!
//! 1. Независимая цепочка (colour-demosaicing Menon 2007 + colour-hdri DNG): XYZ D50 каждого
//!    пикселя совпадает почти точно — уровни, баланс, демозаик и цвет посчитаны чужим кодом.
//! 2. LibRaw (rawpy): другой демозаик (AHD) и другая матрица цвета (LibRaw берёт ColorMatrix,
//!    а не ForwardMatrix, и не интерполирует по температуре) — совпадение приблизительное,
//!    ΔE2000 средних центральной области ограничен и объяснён.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "тесты: падение и есть результат проверки"
)]

mod support;

use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::metrics::{delta_e_2000, display_linear_to_lab, max_abs_difference};
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference};
use support::load_asset;
use support::real_capture::{RealCapture, load_real_capture};

const CROPS: [&str; 4] = ["gray-tile", "towel", "jar", "tap-highlight"];

fn crop_name(crop: &str) -> String {
    format!("real-main-lamp-{crop}")
}

fn run(capture: &RealCapture, gamut: DisplayGamut) -> ReferenceRun {
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let controls =
        ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Quality, gamut).with_legacy_tone();
    let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
    run_reference(&capture.raw, &params).unwrap()
}

#[test]
fn xyz_matches_independent_reference_chain_pixel_for_pixel() {
    for crop in CROPS {
        let capture = load_real_capture(&support::assets_dir(), &crop_name(crop));
        let run = run(&capture, DisplayGamut::DisplayP3);
        let reference = load_asset(&format!("{}-xyz", crop_name(crop)));
        let ours: Vec<f32> = run.xyz_d50.pixels().iter().flatten().copied().collect();
        let difference = max_abs_difference(reference.f32_samples(), &ours).unwrap();
        // f32 против f64 эталона: значения ~0,01…0,3, ошибка округления f32 ~1e-7 на операцию,
        // цепочка ~50 операций → допуск 2e-6 с запасом. Ошибка в любой формуле даёт ≥ 1e-3.
        assert!(difference < 2e-6, "{crop}: max |Δ| XYZ = {difference:e}");
    }
}

#[test]
fn white_point_matches_colour_hdri() {
    let capture = load_real_capture(&support::assets_dir(), &crop_name("gray-tile"));
    let white = capture
        .sensor
        .color_profile()
        .white_for_neutral(capture.neutral)
        .unwrap();
    let reference = capture.asset.field("extra.reference_white_xy");
    let reference_x = reference[0].as_f64().unwrap();
    let reference_y = reference[1].as_f64().unwrap();
    // colour-hdri стартует с (1/3, 1/3) и не ограничивает проходы, мы — с D50 и 30 проходов:
    // точки сходятся к одному решению с точностью порога остановки.
    assert!(
        (white.white.x - reference_x).abs() < 1e-6 && (white.white.y - reference_y).abs() < 1e-6,
        "{white:?}"
    );
}

/// Среднее центральной области изображения (без отступа `margin` от края кропа).
fn inner_mean(pixels: &[[f32; 3]], width: u32, height: u32, margin: u32) -> [f32; 3] {
    let mut sum = [0.0_f64; 3];
    let mut count = 0.0;
    for y in margin..height - margin {
        for x in margin..width - margin {
            let pixel = pixels[(y * width + x) as usize];
            for channel in 0..3 {
                sum[channel] += f64::from(pixel[channel]);
            }
            count += 1.0;
        }
    }
    sum.map(|value| (value / count) as f32)
}

fn manifest_vector<const N: usize>(capture: &RealCapture, field: &str) -> [f32; N] {
    let values = capture.asset.field(field);
    std::array::from_fn(|index| values[index].as_f64().unwrap() as f32)
}

/// ΔE2000 двух линейных sRGB, приведённых к масштабу «серая плитка = 0,5»: кропы тёмные
/// (Y ~ 0,03–0,2), а ΔE должен отражать заметность на нормально экспонированном кадре.
fn delta_e_at_display_scale(reference: [f32; 3], sample: [f32; 3], gray_tile_green: f32) -> f64 {
    let scale = 0.5 / gray_tile_green;
    delta_e_2000(
        display_linear_to_lab(reference.map(|channel| channel * scale), DisplayGamut::Srgb).unwrap(),
        display_linear_to_lab(sample.map(|channel| channel * scale), DisplayGamut::Srgb).unwrap(),
    )
}

/// Сравнение с LibRaw. Расхождение ожидаемо и объяснимо: LibRaw для DNG строит матрицу цвета
/// только из ColorMatrix1 (D65), без ForwardMatrix и интерполяции по температуре (сверено:
/// его `rgb_cam` совпадает с выведенной из CM1 до 1e-4). Кадр снят при ~4530 K, поэтому
/// насыщенные цвета расходятся сильнее серых. Доказательство, что причина именно в матрице:
/// наш же демозаик с матрицей LibRaw совпадает с LibRaw почти точно (остаток — AHD против Menon).
#[test]
fn colour_differs_from_libraw_only_by_its_colour_matrix() {
    let captures: Vec<RealCapture> = CROPS
        .iter()
        .map(|crop| load_real_capture(&support::assets_dir(), &crop_name(crop)))
        .collect();
    let margin = captures[0].asset.field("extra.reference_margin").as_u64().unwrap() as u32;
    let runs: Vec<ReferenceRun> = captures
        .iter()
        .map(|capture| run(capture, DisplayGamut::Srgb))
        .collect();
    let size = runs[0].scene_linear.size();
    let ours: Vec<[f32; 3]> = runs
        .iter()
        .map(|run| inner_mean(run.scene_linear.pixels(), size.width, size.height, margin))
        .collect();
    let libraw: Vec<[f32; 3]> = captures
        .iter()
        .map(|capture| manifest_vector(capture, "extra.libraw_inner_mean_linear_srgb"))
        .collect();
    // LibRaw масштабирует данные по-своему (нормировка множителей и максимума): одна общая
    // экспозиция выравнивается по зелёному серой плитки, цвет при этом не трогается.
    let exposure = ours[0][1] / libraw[0][1];
    for (index, crop) in CROPS.iter().enumerate() {
        let libraw_aligned = libraw[index].map(|channel| channel * exposure);
        let dng_delta = delta_e_at_display_scale(libraw_aligned, ours[index], ours[0][1]);

        let rgb_cam = manifest_vector::<9>(&captures[index], "extra.libraw_rgb_cam");
        let libraw_matrix = isp_model::color::matrix::PixelMatrix {
            rows: [
                [rgb_cam[0], rgb_cam[1], rgb_cam[2]],
                [rgb_cam[3], rgb_cam[4], rgb_cam[5]],
                [rgb_cam[6], rgb_cam[7], rgb_cam[8]],
            ],
        };
        let camera_mean = inner_mean(runs[index].camera_rgb.pixels(), size.width, size.height, margin);
        let replica = libraw_matrix.apply(camera_mean);
        let replica_exposure = libraw_aligned[1] / replica[1];
        let replica_delta = delta_e_at_display_scale(
            libraw_aligned,
            replica.map(|channel| channel * replica_exposure),
            ours[0][1],
        );
        println!("{crop}: ΔE2000 DNG-путь против LibRaw = {dng_delta:.2}, с матрицей LibRaw = {replica_delta:.2}");
        // Измерено: серый 2,2, блик 0,7, полотенце 5,9, банка 6,8; с матрицей LibRaw — 0,01…0,3.
        assert!(dng_delta < 8.0, "{crop}: ΔE2000 против LibRaw = {dng_delta}");
        assert!(
            replica_delta < 0.5,
            "{crop}: с матрицей LibRaw ΔE2000 = {replica_delta}"
        );
    }
}
