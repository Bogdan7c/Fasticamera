//! Гистограмма на GPU (сессия 14s): счётчики == эталон бит в бит, картинка == эталон в допуске fp16, и сама гистограмма
//! говорит правду — серый кадр даёт один столбик на своём уровне во всех плоскостях, красный кадр — красный столбик в
//! верхнем уровне и голубой в нижнем.
//!
//! Матрица картинки: 3 режима × 6 шкал высоты (`HistogramHeight`) × 3 вида рисования (решение 337) × 2 размера картинки
//! (уже и шире сетки) на двух сценах; продуктовый размер 1011 × 827 — отдельным тестом. Положение телефона на гистограмму не влияет: счётчики и картинка при четырёх
//! положениях равны.

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
use scope_contract::{HistogramMode, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{
    HistogramHeight, HistogramLook, RenderSettings, ResolvedScope, SampleStride, ScopeModelSettings, ScopeRule, render,
};
use scope_support::{
    Scene, assert_no_validation_errors, bars75_scene, cpu_counters, gpu_context_or_skip, gray_ramp_scene, picture_size,
    random_scene, rule_cases_for_scan, run_scope_frame, small_settings, upload_scene, worst_premultiplied_difference,
};

/// Допуск картинки: четыре шага fp16 у единицы (как у `scope_picture.rs`).
const PICTURE_TOLERANCE: f32 = 2.0e-3;
const BINS: u32 = 512;

const MODES: [HistogramMode; 3] = [HistogramMode::Rgb, HistogramMode::Parade, HistogramMode::Luma];

/// Все шкалы высоты, которые сравниваются с эталоном.
const HEIGHTS: [HistogramHeight; 6] = [
    HistogramHeight::LinearToPeak,
    HistogramHeight::LinearToMean { multiple: 2.0 },
    HistogramHeight::SquareRootToPeak,
    HistogramHeight::SquareRootToMean { multiple: 6.0 },
    HistogramHeight::Log2ToPeak,
    HistogramHeight::Log2ToMean { multiple: 3.0 },
];

/// Виды рисования, которые сравниваются с эталоном: умолчания продукта (решение 337) и «жирный» вид — широкая линия, плотная
/// заливка без затемнения, заметный зазор между полосами, чтобы ветки линии и зазора не прятались за умолчаниями; и вид без
/// линии верха (толщина 0).
const LOOKS: [HistogramLook; 3] = [
    HistogramLook {
        fill_opacity: 0.28,
        fill_tint: 0.7,
        edge_width_px: 1.25,
        strip_gap: 0.012,
    },
    HistogramLook {
        fill_opacity: 0.6,
        fill_tint: 1.0,
        edge_width_px: 3.0,
        strip_gap: 0.08,
    },
    HistogramLook {
        fill_opacity: 0.28,
        fill_tint: 0.7,
        edge_width_px: 0.0,
        strip_gap: 0.0,
    },
];

fn scopes_for(context: &gpu_vulkan::context::GpuContext, settings: &ScopeModelSettings) -> ColorScopes {
    ColorScopes::new(
        context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap()
}

fn histogram_rule(settings: &ScopeModelSettings, mode: HistogramMode, scan: ScopeScan) -> ResolvedScope {
    ResolvedScope::resolve(
        &ScopeRequest {
            kind: ScopeKind::Histogram,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
            cie_diagram: scope_contract::CieDiagram::default(),
            histogram_mode: mode,
            scan,
            image_size: picture_size(160, 96),
        },
        settings,
        isp_model::image::DisplayGamut::Srgb,
    )
    .unwrap()
}

fn solid_scene(name: &'static str, code: [u16; 3]) -> Scene {
    Scene {
        name,
        width: 64,
        height: 32,
        codes: vec![code; 64 * 32],
    }
}

/// Шаг 1 по всем осям: каждый пиксель сцены в счётчиках (числа проверяются точно).
fn every_pixel_settings() -> ScopeModelSettings {
    ScopeModelSettings {
        histogram_sample_stride: SampleStride::EVERY_PIXEL,
        ..small_settings()
    }
}

#[test]
fn a_gray_frame_gives_one_bar_at_the_gray_level_in_every_plane_and_in_luma() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = every_pixel_settings();
    let mut scopes = scopes_for(&context, &settings);
    for code in [0_u16, 300, 512, 1023] {
        let scene = solid_scene("solid-gray", [code; 3]);
        let input = upload_scene(&context, &scene, StorageFormatPolicy::HalfFloatOnly, (5, 3));
        let level = u32::from(code) * BINS / 1024;
        for mode in MODES {
            let rule = histogram_rule(&settings, mode, ScopeScan::Upright);
            let frame = ScopeFrame {
                input: input.input(HorizontalMirror::Off),
                rule: &rule,
                settings: &settings,
                picture_size: picture_size(160, 96),
                frame_slot: 0,
            };
            run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let gpu = scopes.read_back_counters_blocking().unwrap();
            let pixels = 64 * 32;
            let planes = if mode == HistogramMode::Luma { 1 } else { 3 };
            assert_eq!(
                (gpu.shape().columns(), gpu.shape().rows(), gpu.shape().planes()),
                (BINS, 1, planes)
            );
            for plane in 0..planes {
                for bin in 0..BINS {
                    let expected = if bin == level { pixels } else { 0 };
                    assert_eq!(
                        gpu.count(plane, 0, bin),
                        Some(expected),
                        "код {code}, {mode:?}, плоскость {plane}, уровень {bin}"
                    );
                }
            }
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn a_saturated_red_frame_fills_the_top_level_of_red_and_the_bottom_level_of_green_and_blue() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = every_pixel_settings();
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &solid_scene("solid-red", [1023, 0, 0]),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let rule = histogram_rule(&settings, HistogramMode::Rgb, ScopeScan::Upright);
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: &rule,
        settings: &settings,
        picture_size: picture_size(160, 96),
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let gpu = scopes.read_back_counters_blocking().unwrap();
    let pixels = 64 * 32;
    assert_eq!(gpu.count(0, 0, BINS - 1), Some(pixels), "красный — в верхнем уровне");
    assert_eq!(gpu.count(1, 0, 0), Some(pixels), "зелёный — в нижнем");
    assert_eq!(gpu.count(2, 0, 0), Some(pixels), "синий — в нижнем");
    assert_eq!(gpu.total(), 3 * u64::from(pixels));

    // Картинка с GPU (вид решения 337 по умолчанию): совпадает с эталоном; правый конец — красный, левый — голубой.
    let render_settings = RenderSettings {
        graticule_opacity: 0.0,
        histogram_graticule_opacity: 0.0,
        histogram_height: HistogramHeight::LinearToPeak,
        ..RenderSettings::default()
    };
    let drawn = ScopeModelSettings {
        render: render_settings.clone(),
        ..settings.clone()
    };
    let frame = ScopeFrame {
        settings: &drawn,
        ..frame
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let readback = scopes.read_back_picture_blocking().unwrap();
    let samples = rule.planned_samples(64, 32, &drawn);
    let cpu = render(&gpu, &rule, &render_settings, picture_size(160, 96), samples).unwrap();
    let worst = worst_premultiplied_difference(&readback.texels, &cpu);
    assert!(
        worst <= PICTURE_TOLERANCE,
        "картинка красного кадра против эталона: {worst}"
    );
    let texel = |x: u32, y: u32| readback.texels[(y * readback.size.width() + x) as usize];
    // Линия верха: у правого красного столбика достаёт до верха, цвет — красный; у левого — зелёный и синий вместе.
    let top_right = (0..96).map(|y| texel(158, y)).find(|pixel| pixel[3] > 0.99).unwrap();
    assert!(
        top_right[0] > top_right[1] && top_right[0] > top_right[2],
        "правый — красный: {top_right:?}"
    );
    let top_left = (0..96).map(|y| texel(1, y)).find(|pixel| pixel[3] > 0.99).unwrap();
    assert!(
        top_left[0] < top_left[1] && top_left[0] < top_left[2],
        "левый — голубой: {top_left:?}"
    );
    // Заливка полупрозрачна: у основания видно картинку, а не сплошную массу.
    assert!(
        texel(158, 94)[3] < 0.99 && texel(158, 94)[3] > 0.0,
        "{:?}",
        texel(158, 94)
    );
    assert_eq!(texel(80, 95), [0.0; 4], "между столбиками пусто");
    assert_no_validation_errors(&context);
}

#[test]
fn picture_matches_the_cpu_reference_in_every_mode_height_scale_and_picture_width() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base_settings = small_settings();
    let mut scopes = scopes_for(&context, &base_settings);
    let mut worst_overall = 0.0_f32;
    let mut compared = 0;
    // Две сцены: случайная занимает все уровни ровно, полосы 75 % — восемь резких пиков и пустота между ними (опора
    // «самый высокий столбик» и кривые на разреженной гистограмме).
    for scene in [random_scene(0xC0FF_EE11), bars75_scene()] {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (3, 2));
        let shown = input.shown_codes(HorizontalMirror::Off);
        for size in [picture_size(160, 96), picture_size(1011, 64)] {
            for mode in MODES {
                for (height, look) in HEIGHTS.into_iter().flat_map(|height| LOOKS.map(|look| (height, look))) {
                    let settings = ScopeModelSettings {
                        render: RenderSettings {
                            histogram_height: height,
                            histogram_fill_opacity: look.fill_opacity,
                            histogram_fill_tint: look.fill_tint,
                            histogram_edge_width_px: look.edge_width_px,
                            histogram_strip_gap: look.strip_gap,
                            ..base_settings.render.clone()
                        },
                        ..base_settings.clone()
                    };
                    let rule = histogram_rule(&settings, mode, ScopeScan::Upright);
                    let frame = ScopeFrame {
                        input: input.input(HorizontalMirror::Off),
                        rule: &rule,
                        settings: &settings,
                        picture_size: size,
                        frame_slot: 0,
                    };
                    run_scope_frame(&context, &mut scopes, &frame).unwrap();
                    let counters = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
                    assert_eq!(
                        scopes.read_back_counters_blocking().unwrap(),
                        counters,
                        "{mode:?}: счётчики"
                    );
                    let samples = rule.planned_samples(shown.width(), shown.height(), &settings);
                    let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
                    let readback = scopes.read_back_picture_blocking().unwrap();
                    let worst = worst_premultiplied_difference(&readback.texels, &cpu);
                    eprintln!(
                        "гистограмма {} / {mode:?} / {height:?} / {look:?} / {}×{}: расхождение {worst:.2e}",
                        scene.name,
                        size.width(),
                        size.height()
                    );
                    assert!(
                        worst <= PICTURE_TOLERANCE,
                        "{mode:?} / {height:?} / {look:?} / {}×{}: расхождение {worst} больше допуска",
                        size.width(),
                        size.height()
                    );
                    // Столбики настоящие: след занимает заметную долю картинки и не вся картинка закрашена.
                    let covered = readback.texels.iter().filter(|pixel| pixel[3] > 0.0).count();
                    assert!(covered > 50, "{mode:?} / {height:?}: столбики есть ({covered})");
                    assert!(
                        covered < readback.texels.len(),
                        "{mode:?} / {height:?}: картинка не залита целиком"
                    );
                    worst_overall = worst_overall.max(worst);
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 2 * 2 * 3 * 6 * LOOKS.len(), "размер матрицы");
    eprintln!("наибольшее расхождение картинки гистограммы: {worst_overall:.2e}");
    assert_no_validation_errors(&context);
}

/// Продуктовый размер картинки (1011 × 827, три полосы PARADE по ~270 px) во всех режимах с видом по умолчанию.
#[test]
fn the_product_sized_picture_matches_the_cpu_reference_in_every_mode() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings);
    let scene = random_scene(0x1011_0827);
    let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (3, 2));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let size = picture_size(1011, 827);
    for mode in MODES {
        let rule = histogram_rule(&settings, mode, ScopeScan::Upright);
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: size,
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
        let samples = rule.planned_samples(shown.width(), shown.height(), &settings);
        let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
        let worst = worst_premultiplied_difference(&scopes.read_back_picture_blocking().unwrap().texels, &cpu);
        assert!(worst <= PICTURE_TOLERANCE, "{mode:?}: расхождение {worst}");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn the_phone_orientation_changes_neither_the_counters_nor_the_picture_of_the_histogram() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings);
    // Случайная сцена 97 × 61 несимметрична: любое чтение «слева направо» дало бы другую картинку счётчиков.
    let input = upload_scene(
        &context,
        &random_scene(0xFACE_B00C),
        StorageFormatPolicy::HalfFloatOnly,
        (2, 2),
    );
    let mut seen = Vec::new();
    for scan in [
        ScopeScan::Upright,
        ScopeScan::RotatedClockwise,
        ScopeScan::UpsideDown,
        ScopeScan::RotatedCounterClockwise,
    ] {
        for (name, rule) in rule_cases_for_scan(&settings, scan) {
            if !name.starts_with("histogram") {
                continue;
            }
            let frame = ScopeFrame {
                input: input.input(HorizontalMirror::Off),
                rule: &rule,
                settings: &settings,
                picture_size: picture_size(160, 96),
                frame_slot: 0,
            };
            run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let counters = scopes.read_back_counters_blocking().unwrap();
            assert!(counters.total() > 0);
            let picture = scopes.read_back_picture_blocking().unwrap().texels;
            seen.push((name, scan, counters, picture));
        }
    }
    assert_eq!(seen.len(), 4 * 3, "четыре положения × три режима");
    for (name, scan, counters, picture) in &seen {
        let (_, _, upright_counters, upright_picture) = seen
            .iter()
            .find(|(other, other_scan, _, _)| other == name && *other_scan == ScopeScan::Upright)
            .unwrap();
        assert_eq!(counters, upright_counters, "{name} при {scan:?}: счётчики");
        assert_eq!(picture, upright_picture, "{name} при {scan:?}: картинка");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn a_gray_ramp_over_all_codes_fills_every_level_evenly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = every_pixel_settings();
    let mut scopes = scopes_for(&context, &settings);
    let scene = gray_ramp_scene();
    let input = upload_scene(&context, &scene, StorageFormatPolicy::HalfFloatOnly, (0, 0));
    for mode in MODES {
        let rule = histogram_rule(&settings, mode, ScopeScan::Upright);
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: picture_size(160, 96),
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = scopes.read_back_counters_blocking().unwrap();
        // 1024 кода × 3 строки на 512 уровней: по 2 кода × 3 строки = 6 отсчётов на каждый уровень каждой плоскости.
        for plane in 0..counters.shape().planes() {
            for bin in 0..BINS {
                assert_eq!(
                    counters.count(plane, 0, bin),
                    Some(6),
                    "{mode:?}, плоскость {plane}, уровень {bin}"
                );
            }
        }
    }
    assert_no_validation_errors(&context);
}

/// Опора шкалы «кратное среднего» считается из отсчётов ТОГО кадра: окно уменьшилось между кадрами — опора ушла за ним, а
/// картинка по отсчётам прошлого окна заметно отличалась бы от настоящей.
#[test]
fn the_mean_reference_follows_the_window_of_each_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        render: RenderSettings {
            histogram_height: HistogramHeight::LinearToMean { multiple: 2.0 },
            ..small_settings().render
        },
        ..small_settings()
    };
    let mut scopes = scopes_for(&context, &settings);
    let scene = random_scene(0xA11C_E5ED);
    let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (2, 2));
    let size = picture_size(160, 96);
    let rule = histogram_rule(&settings, HistogramMode::Rgb, ScopeScan::Upright);
    let mut previous: Option<(scope_model::SampleCount, scope_model::ScopeCounters)> = None;
    for (width, height) in [(97, 61), (48, 30), (97, 61)] {
        let frame = ScopeFrame {
            input: isp_vulkan::scopes::ScopeInput {
                view: input.image.sampled_view(),
                window: gpu_vulkan::source_window::SourceWindow::of_content(ash::vk::Extent2D { width, height }),
            },
            rule: &rule,
            settings: &settings,
            picture_size: size,
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = scopes.read_back_counters_blocking().unwrap();
        let samples = rule.planned_samples(width, height, &settings);
        let gpu = scopes.read_back_picture_blocking().unwrap();
        let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
        let worst = worst_premultiplied_difference(&gpu.texels, &cpu);
        assert!(worst <= PICTURE_TOLERANCE, "окно {width}×{height}: расхождение {worst}");
        if let Some((stale_samples, _)) = previous.as_ref().filter(|(stale, _)| *stale != samples) {
            let stale_cpu = render(&counters, &rule, &settings.render, size, *stale_samples).unwrap();
            let stale_worst = worst_premultiplied_difference(&gpu.texels, &stale_cpu);
            assert!(
                stale_worst > PICTURE_TOLERANCE * 5.0,
                "окно {width}×{height}: картинка по отсчётам прошлого окна не отличима ({stale_worst})"
            );
        }
        previous = Some((samples, counters));
    }
    assert_no_validation_errors(&context);
}
