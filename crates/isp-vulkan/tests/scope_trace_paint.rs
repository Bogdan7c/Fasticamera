//! Способы рисования следа на GPU против эталона `scope-model` и по смыслу картинки (решения 264–267):
//! наложение трёх плоскостей (RGB waveform), цвет вектороскопа по месту, опорное число яркости от доли отсчётов.
//!
//! * опорное число `trace.x` пересобирается КАЖДЫЙ кадр: окно показанной картинки меняется между кадрами, и
//!   картинка GPU совпадает с эталоном, посчитанным по ЧИСЛУ ОТСЧЁТОВ ТЕКУЩЕГО окна (а не прошлого);
//! * на цветных полосах RGB waveform даёт смешанные цвета (жёлтый, голубой, пурпурный, белый), а не секции;
//! * серый кадр красит центр вектороскопа нейтралью, и картинка конечна (ветка «r = 0» до вычисления угла);
//! * цвета вектороскопа по месту сходятся с цветом кода, который лёг в эту клетку.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use ash::vk;
use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame, ScopeInput};
use scope_model::{
    POSITION_PALETTE_ANGLES, PositionPalette, PositionPaletteEntry, RgbWaveformPalette, SampleCount,
    ScopeModelSettings, TraceStyle, hits_of_code, render,
};
use scope_support::{
    PaintOverride, Scene, Xorshift, assert_no_validation_errors, bars75_scene, gpu_context_or_skip, gradient_scene,
    picture_size, rule_cases, run_scope_frame, small_settings, upload_scene,
};

/// Допуск сравнения картинки (тот же, что у `scope_picture.rs`).
const PICTURE_TOLERANCE: f32 = 2.0e-3;

/// Большой случайный кадр: на маленьких кадрах порог яркости упирается в нижнюю границу «одно попадание» и
/// число отсчётов ни на что не влияет, а тест обязан это число различать.
fn big_random_scene() -> Scene {
    let (width, height) = (800, 600);
    let mut random = Xorshift(0x00C0_FFEE);
    let codes = (0..width * height)
        .map(|_| [0, 1, 2].map(|_| (random.next() % 1024) as u16))
        .collect();
    Scene {
        name: "random-800x600",
        width,
        height,
        codes,
    }
}

fn premultiplied_difference(gpu: &[[f32; 4]], cpu: &[[f32; 4]]) -> f32 {
    assert_eq!(gpu.len(), cpu.len());
    let premultiplied = |pixel: [f32; 4]| [pixel[0] * pixel[3], pixel[1] * pixel[3], pixel[2] * pixel[3], pixel[3]];
    gpu.iter()
        .zip(cpu)
        .flat_map(|(gpu_pixel, cpu_pixel)| {
            let (gpu_value, cpu_value) = (premultiplied(*gpu_pixel), premultiplied(*cpu_pixel));
            (0..4).map(move |channel| (gpu_value[channel] - cpu_value[channel]).abs())
        })
        .fold(0.0, f32::max)
}

/// Те же настройки без сетки: цвет следа в пикселе не смешивается с цветом линий (уровень 75 % у полос лежит
/// ровно на линии сетки).
fn without_graticule(mut settings: ScopeModelSettings) -> ScopeModelSettings {
    settings.render.graticule_opacity = 0.0;
    settings
}

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

#[test]
fn reference_brightness_follows_the_window_of_each_frame_and_never_goes_stale() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_for(&context, &settings);
    let scene = big_random_scene();
    let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (4, 2));
    let size = picture_size(160, 96);
    for (rule_name, rule) in rule_cases(&settings) {
        let mut previous_samples: Option<SampleCount> = None;
        // Окно меняется между кадрами с тем же набором ресурсов: сначала полное, потом вчетверо меньше, потом снова полное.
        for (width, height) in [(800, 600), (400, 300), (800, 600)] {
            let frame = ScopeFrame {
                input: ScopeInput {
                    view: input.image.sampled_view(),
                    window: SourceWindow::of_content(vk::Extent2D { width, height }),
                },
                rule: &rule,
                settings: &settings,
                picture_size: size,
                frame_slot: 0,
            };
            run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let counters = scopes.read_back_counters_blocking().unwrap();
            let gpu = scopes.read_back_picture_blocking().unwrap();
            let samples = SampleCount::for_window(width, height, settings.sample_stride);
            let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
            let worst = premultiplied_difference(&gpu.texels, cpu.pixels());
            assert!(
                worst <= PICTURE_TOLERANCE,
                "{rule_name} / окно {width}×{height}: расхождение {worst}"
            );
            // Тест различает число отсчётов: картинка по отсчётам ПРОШЛОГО окна заметно отличалась бы от GPU.
            if let Some(stale) = previous_samples.filter(|stale| *stale != samples) {
                let stale_cpu = render(&counters, &rule, &settings.render, size, stale).unwrap();
                let stale_worst = premultiplied_difference(&gpu.texels, stale_cpu.pixels());
                assert!(
                    stale_worst > PICTURE_TOLERANCE * 5.0,
                    "{rule_name} / окно {width}×{height}: устаревшее число отсчётов не отличимо ({stale_worst})"
                );
            }
            previous_samples = Some(samples);
        }
    }
    assert_no_validation_errors(&context);
}

/// Пиксель GPU-картинки.
fn texel(readback: &isp_vulkan::scopes::PictureReadback, x: u32, y: u32) -> [f32; 4] {
    readback.texels[(y * readback.size.width() + x) as usize]
}

#[test]
fn rgb_waveform_on_colour_bars_mixes_primaries_where_traces_overlap() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = without_graticule(small_settings());
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &bars75_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let size = picture_size(194, 128);
    let cases = rule_cases(&settings);
    let (_, waveform) = cases.iter().find(|(name, _)| *name == "waveform").unwrap();
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: waveform,
        settings: &settings,
        picture_size: size,
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let readback = scopes.read_back_picture_blocking().unwrap();
    // Класс цвета у ярких пикселей следа (сетка рисуется светло-серым и ярких «цветных» пикселей не даёт).
    let primaries = RgbWaveformPalette::Softened.primaries();
    let normalized = |sum: [f32; 3]| {
        let peak = sum.iter().copied().fold(0.0, f32::max);
        sum.map(|channel| channel / peak)
    };
    let add = |first: [f32; 3], second: [f32; 3]| [0, 1, 2].map(|channel| first[channel] + second[channel]);
    let expected = [
        ("красный", normalized(primaries[0])),
        ("зелёный", normalized(primaries[1])),
        ("синий", normalized(primaries[2])),
        ("жёлтый", normalized(add(primaries[0], primaries[1]))),
        ("голубой", normalized(add(primaries[1], primaries[2]))),
        ("пурпурный", normalized(add(primaries[0], primaries[2]))),
        ("белый", [1.0; 3]),
    ];
    for (name, colour) in expected {
        let found = readback
            .texels
            .iter()
            .any(|pixel| pixel[3] > 0.5 && (0..3).all(|channel| (pixel[channel] - colour[channel]).abs() < 0.03));
        assert!(found, "на картинке нет пикселей цвета «{name}» {colour:?}");
    }
    assert!(
        readback
            .texels
            .iter()
            .all(|pixel| pixel.iter().all(|channel| channel.is_finite()))
    );
    assert_no_validation_errors(&context);
}

#[test]
fn pure_palette_changes_the_gpu_trace_colours_through_the_settings() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut settings = without_graticule(small_settings());
    settings.render.rgb_waveform_palette = RgbWaveformPalette::Pure;
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &bars75_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let cases = rule_cases(&settings);
    let (_, waveform) = cases.iter().find(|(name, _)| *name == "waveform").unwrap();
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: waveform,
        settings: &settings,
        picture_size: picture_size(194, 128),
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let readback = scopes.read_back_picture_blocking().unwrap();
    // Чистые основные: настоящий красный (1, 0, 0) и настоящий зелёный есть на картинке (у «смягчённой» палитры
    // у одного красного канала остаются ненулевые зелёный и синий).
    for (name, colour) in [
        ("красный", [1.0, 0.0, 0.0]),
        ("зелёный", [0.0, 1.0, 0.0]),
        ("синий", [0.0, 0.0, 1.0]),
    ] {
        let found = readback
            .texels
            .iter()
            .any(|pixel| pixel[3] > 0.5 && (0..3).all(|channel| (pixel[channel] - colour[channel]).abs() < 0.03));
        assert!(found, "при чистой палитре нет пикселей цвета «{name}»");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn gray_frame_paints_the_vectorscope_centre_neutral_and_finite_on_the_gpu() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = without_graticule(small_settings());
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &gradient_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    // Картинка = сетка 128×128: пиксель (64, 64) — центральная клетка, где d = (0, 0) и угол не определён.
    let size = picture_size(128, 128);
    for (rule_name, rule) in rule_cases(&settings) {
        if !rule_name.starts_with("vectorscope") {
            continue;
        }
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: size,
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let readback = scopes.read_back_picture_blocking().unwrap();
        assert!(
            readback
                .texels
                .iter()
                .all(|pixel| pixel.iter().all(|channel| channel.is_finite())),
            "{rule_name}: в картинке есть NaN или бесконечность"
        );
        let centre = texel(&readback, 64, 64);
        assert!(
            centre[3] > 0.5,
            "{rule_name}: серый кадр рисует яркую точку в центре: {centre:?}"
        );
        let neutral = rule.trace_style(&settings.render);
        let scope_model::TraceStyle::ColorByPosition { palette } = neutral else {
            panic!("{rule_name}: вектороскоп красится по положению");
        };
        for channel in 0..3 {
            assert!(
                (centre[channel] - palette.neutral()[channel]).abs() < 2e-3,
                "{rule_name}: центр {centre:?}, нейтраль {:?}",
                palette.neutral()
            );
        }
    }
    assert_no_validation_errors(&context);
}

/// Ветка «центр» шейдера стоит ДО вычисления угла: `atan(0, 0)` в GLSL не определён (на Adreno может дать NaN). На
/// RADV он, вероятно, даёт 0, поэтому обычный серый кадр ветку не проверяет; отравленная таблица (NaN во всех
/// записях) проверяет: центральная клетка не имеет права читать таблицу вовсе, иначе NaN попадёт в картинку.
#[test]
fn the_vectorscope_centre_never_reads_the_angle_table_on_the_gpu() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = without_graticule(small_settings());
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &gradient_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let cases = rule_cases(&settings);
    let (_, vectorscope) = cases.iter().find(|(name, _)| *name == "vectorscope-ycbcr").unwrap();
    let poisoned_entry = PositionPaletteEntry {
        edge_colour: [f32::NAN; 3],
        edge_radius_cells: f32::NAN,
    };
    let neutral = [0.7, 0.6, 0.5];
    // Сетка вектороскопа 128×128: центр — клетка 64, координата 64,5.
    let palette = PositionPalette::new([poisoned_entry; POSITION_PALETTE_ANGLES], neutral, 64.5);
    let rule = PaintOverride {
        inner: vectorscope,
        style: TraceStyle::ColorByPosition {
            palette: Box::new(palette),
        },
    };
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: &rule,
        settings: &settings,
        picture_size: picture_size(128, 128),
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let readback = scopes.read_back_picture_blocking().unwrap();
    // Серый градиент попадает только в центральную клетку: она светится нейтралью, остальные пиксели прозрачны.
    let centre = texel(&readback, 64, 64);
    assert!(centre[3] > 0.5, "центр светится: {centre:?}");
    for channel in 0..3 {
        assert!(
            (centre[channel] - neutral[channel]).abs() < 2e-3,
            "центр {centre:?}, нейтраль {neutral:?}"
        );
    }
    assert!(
        readback
            .texels
            .iter()
            .all(|pixel| pixel.iter().all(|channel| channel.is_finite())),
        "отравленная таблица не должна просочиться в картинку"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn vectorscope_cell_colour_on_the_gpu_matches_the_code_that_landed_there() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = without_graticule(small_settings());
    let mut scopes = scopes_for(&context, &settings);
    let input = upload_scene(
        &context,
        &bars75_scene(),
        StorageFormatPolicy::PreferTenBitUnorm,
        (0, 0),
    );
    let size = picture_size(128, 128);
    for (rule_name, rule) in rule_cases(&settings) {
        if !rule_name.starts_with("vectorscope") {
            continue;
        }
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: size,
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let readback = scopes.read_back_picture_blocking().unwrap();
        let on = 767_u16;
        for (name, code, active) in [
            ("красный", [on, 0, 0], [true, false, false]),
            ("зелёный", [0, on, 0], [false, true, false]),
            ("синий", [0, 0, on], [false, false, true]),
            ("жёлтый", [on, on, 0], [true, true, false]),
        ] {
            let hit = hits_of_code(&rule, scope_model::SamplePosition { x: 0, picture_width: 1 }, code)[0];
            let pixel = texel(&readback, hit.column, hit.row);
            assert!(
                pixel[3] > 0.5,
                "{rule_name} / {name}: клетка ({}, {}) должна светиться",
                hit.column,
                hit.row
            );
            let on_min = (0..3)
                .filter(|&c| active[c])
                .map(|c| pixel[c])
                .fold(f32::INFINITY, f32::min);
            let off_max = (0..3)
                .filter(|&c| !active[c])
                .map(|c| pixel[c])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                on_min > off_max + 0.05,
                "{rule_name} / {name}: цвет {pixel:?} не того оттенка"
            );
        }
    }
    assert_no_validation_errors(&context);
}
