//! Вертикальные полосы следа RGB waveform и parade: корень и защита (решение 322).
//!
//! Корень (доказан на живых скринах владельца в 14r): сетка была фиксированной — 512 столбцов, а отсчётов вдоль
//! взгляда бывает меньше. В разделе калибровки видоискатель на весь экран, окно, которое меряет прибор, ≈ 917 px
//! шириной; при шаге 2 — 458 отсчётов на 512 столбцов, 54 столбца без единого отсчёта — тёмные полосы через
//! ≈ 19 px (waveform) и ≈ 6,3 px (parade). Масштаб столбца из 14m выравнивал только НЕРАВНЫЕ столбцы, пустые он
//! поправить не мог. Теперь столбец сетки — номер отсчёта вдоль взгляда: у каждого столбца ровно столько отсчётов,
//! сколько поперёк взгляда, а в пиксели картинки сетка переносится ядром, сохраняющим попадания.
//!
//! Тест: картинка, в каждом столбце которой одно и то же (уровень растёт сверху вниз), обязана дать РОВНЫЙ по ширине
//! след — при окнах уже и шире прежней сетки, при шаге 1 и 2, при всех четырёх положениях телефона, на картинке
//! той же пропорции, что в разделе (917 → 1011). На коде до 14r окно 459 px при шаге 2 давало пустые столбцы.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{flat, picture};
use isp_model::image::DisplayGamut;
use scope_contract::{ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{
    RenderSettings, ResolvedScope, SampleCount, SampleSteps, SampleStride, ScopeModelSettings, ScopeRule, ShownCodes,
    accumulate, render,
};

/// Отсчётов поперёк взгляда (высота окна в портрете): мало, чтобы тест был быстрым, но достаточно для рисунка.
const ACROSS_EYE: u32 = 24;
/// Высота картинки прибора.
const PICTURE_HEIGHT: u32 = 48;

fn resolve(kind: ScopeKind, scan: ScopeScan) -> ResolvedScope {
    let request = ScopeRequest {
        kind,
        vectorscope_axes: VectorscopeAxes::YCbCrBt709,
        vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
        cie_diagram: scope_contract::CieDiagram::default(),
        histogram_mode: scope_contract::HistogramMode::Rgb,
        scan,
        image_size: ScopeImageSize::try_new(64, 64).unwrap(),
    };
    ResolvedScope::resolve(&request, &ScopeModelSettings::default(), DisplayGamut::Srgb).unwrap()
}

/// Окно, в котором вдоль взгляда `eye_length` пикселей, а поперёк — [`ACROSS_EYE`]; каждый столбец взгляда одинаков:
/// уровень зависит только от положения поперёк взгляда.
fn ramp_window(eye_length: u32, scan: ScopeScan) -> ShownCodes {
    let level_of = |across: u32| (across * 1023 / (ACROSS_EYE - 1)) as u16;
    let code = |across: u32| {
        let level = level_of(across);
        [level, 1023 - level, level / 2 + 256]
    };
    if scan.runs_along_surface_rows() {
        picture(ACROSS_EYE, eye_length, |x, _| code(x))
    } else {
        picture(eye_length, ACROSS_EYE, |_, y| code(y))
    }
}

/// Сумма альфы следа по каждому столбцу картинки шириной `width` (у parade — только секция первой плоскости).
fn alpha_profile(kind: ScopeKind, scan: ScopeScan, eye_length: u32, stride: SampleStride, width: u32) -> Vec<f32> {
    let scope = resolve(kind, scan);
    let window = ramp_window(eye_length, scan);
    let counters = accumulate(&window, stride, &scope).unwrap();
    let samples = SampleCount::for_window(window.width(), window.height(), stride);
    let settings = RenderSettings {
        graticule_opacity: 0.0,
        ..RenderSettings::default()
    };
    let size = ScopeImageSize::try_new(width, PICTURE_HEIGHT).unwrap();
    let picture = render(&counters, &scope, &settings, size, samples).unwrap();
    let columns = if kind == ScopeKind::RgbParade { width / 3 } else { width };
    (0..columns)
        .map(|x| (0..PICTURE_HEIGHT).map(|y| picture.pixel(x, y).unwrap()[3]).sum())
        .collect()
}

/// Самый тёмный и самый яркий столбец относительно медианы, без краёв (у края ядро честно теряет массу за краем).
fn darkest_and_brightest_to_median(profile: &[f32]) -> (f32, f32) {
    let inner = &profile[6..profile.len() - 6];
    let mut sorted = inner.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    (sorted[0] / median, sorted[sorted.len() - 1] / median)
}

#[test]
fn a_window_narrower_than_the_old_fixed_grid_gives_no_dark_columns_at_any_scan_or_stride() {
    // Пропорция раздела: окно 917 → картинка 1011 (здесь вдвое меньше: 459 → 506), зум 1,7× (окно 270), фронталка
    // (окно 432), окно шире картинки (1200 → 506, уменьшение) и окно ровно в картинку.
    for (eye_length, width) in [(459, 506), (270, 506), (432, 506), (1200, 506), (506, 506)] {
        for stride in [SampleStride::EVERY_PIXEL, SampleStride::EVERY_SECOND_PIXEL] {
            for scan in ScopeScan::ALL {
                for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
                    let parade_width = if kind == ScopeKind::RgbParade { width * 3 } else { width };
                    let profile = alpha_profile(kind, scan, eye_length, stride, parade_width);
                    let (darkest, brightest) = darkest_and_brightest_to_median(&profile);
                    let what = format!("{kind:?} {scan:?}, окно {eye_length}, шаг {}", stride.get());
                    // Пустой столбец дал бы ~0; неравный — ~±30 %. Ровный след — в пределах 3 %.
                    assert!(darkest > 0.97, "{what}: тёмный столбец {darkest}");
                    assert!(brightest < 1.03, "{what}: яркий столбец {brightest}");
                }
            }
        }
    }
}

#[test]
fn the_calibration_section_geometry_on_the_phone_draws_an_even_trace() {
    // Ровно телефон 14o: окно 917 px вдоль взгляда, картинка RGB waveform 1011 px, шаг продукта (1).
    let profile = alpha_profile(
        ScopeKind::Waveform,
        ScopeScan::Upright,
        917,
        SampleStride::EVERY_PIXEL,
        1011,
    );
    let (darkest, brightest) = darkest_and_brightest_to_median(&profile);
    assert!(darkest > 0.98 && brightest < 1.02, "{darkest} … {brightest}");
}

/// Картинка следа ровного кадра `window` при шагах `steps` (сетка прибора не рисуется — только след).
fn trace_picture(
    scope: &ResolvedScope,
    window: &ShownCodes,
    steps: SampleSteps,
    width: u32,
) -> scope_model::ScopePicture {
    let counters = accumulate(window, steps, scope).unwrap();
    let samples = SampleCount::for_window(window.width(), window.height(), steps);
    let settings = RenderSettings {
        graticule_opacity: 0.0,
        ..RenderSettings::default()
    };
    let size = ScopeImageSize::try_new(width, PICTURE_HEIGHT).unwrap();
    render(&counters, scope, &settings, size, samples).unwrap()
}

#[test]
fn every_second_sample_across_the_eye_keeps_the_trace_brightness_of_every_sample() {
    // Решение 329: поперёк взгляда — каждый второй отсчёт. Каждый столбец получает вдвое меньше попаданий, но и
    // равномерная плотность вдвое меньше, а яркость следа — от их отношения (решение 266): картинка та же.
    let every = SampleStride::EVERY_PIXEL;
    let second = SampleStride::EVERY_SECOND_PIXEL;
    for kind in [ScopeKind::Waveform, ScopeKind::LumaWaveform, ScopeKind::RgbParade] {
        for scan in ScopeScan::ALL {
            let scope = resolve(kind, scan);
            // Ровный тёплый серый: вдоль взгляда 300 px, поперёк 64 (ровно делится на 2 — поровну отсчётов).
            let (width, height) = if scan.runs_along_surface_rows() {
                (64, 300)
            } else {
                (300, 64)
            };
            let window = flat(width, height, [700, 600, 500]);
            let full = trace_picture(&scope, &window, SampleSteps::for_eye(every, every, scan), 330);
            let halved = trace_picture(&scope, &window, SampleSteps::for_eye(every, second, scan), 330);
            let largest_difference = full
                .pixels()
                .iter()
                .zip(halved.pixels())
                .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
                .fold(0.0_f32, f32::max);
            let brightest = full.pixels().iter().map(|pixel| pixel[3]).fold(0.0_f32, f32::max);
            assert!(brightest > 0.1, "{kind:?} {scan:?}: след виден");
            assert!(
                largest_difference < 1e-4,
                "{kind:?} {scan:?}: картинка разошлась на {largest_difference}"
            );
        }
    }
}

#[test]
fn the_product_plan_of_column_traces_takes_every_column_and_every_second_row_of_the_section_window() {
    // Окно раздела на телефоне 917 × 2048 при настройках продукта (решения 322 и 329).
    let settings = ScopeModelSettings::default();
    for kind in [ScopeKind::Waveform, ScopeKind::LumaWaveform, ScopeKind::RgbParade] {
        for scan in ScopeScan::ALL {
            let scope = resolve(kind, scan);
            let plan = scope.planned_samples(917, 2048, &settings);
            let (along, across) = if scan.runs_along_surface_rows() {
                (2048, 459)
            } else {
                (917, 1024)
            };
            assert_eq!(
                (plan.along_eye(scan), plan.across_eye(scan)),
                (along, across),
                "{kind:?} {scan:?}: все столбцы, поперёк — каждый второй"
            );
            // Столбцов сетки — столько же, сколько отсчётов вдоль взгляда (у parade тоже: плоскости — отдельные
            // слои сетки, а не столбцы): пустых столбцов нет.
            assert_eq!(scope.shape(plan).columns(), along, "{kind:?} {scan:?}");
        }
    }
    // Вектороскоп — свой шаг 2 по обеим осям (решение 328), шаг поперёк взгляда следа его не касается.
    let vectorscope = resolve(ScopeKind::Vectorscope, ScopeScan::Upright);
    assert_eq!(
        vectorscope.planned_samples(917, 2048, &settings).steps(),
        SampleSteps::uniform(SampleStride::EVERY_SECOND_PIXEL)
    );
}
