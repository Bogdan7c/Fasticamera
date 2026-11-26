//! Картинка прибора на GPU против `scope_model::render` (CPU-эталон показа).
//!
//! # Допуск и откуда расхождение
//! Отображение «пиксель → ячейка» целочисленное и совпадает с эталоном ТОЧНО: какая ячейка и какой цвет плоскости
//! берутся, расходиться не может. Расходится арифметика яркости: на GPU `log2` (точность Vulkan — несколько ULP),
//! деление и `sqrt` покрытия сетки, а результат пишется в fp16 (11 бит мантиссы: шаг у значений 0,5…1 —
//! `4,9·10⁻⁴`, драйвер вправе округлять к нулю). Поэтому сравниваются ПРЕМУЛЬТИПЛИЦИРОВАННЫЕ значения
//! (`rgb·a` и `a`: прямой цвет при почти нулевой альфе — шум деления) с допуском `PICTURE_TOLERANCE` = 2·10⁻³
//! (четыре шага fp16 у единицы; замеренный максимум печатается тестом).

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
use scope_model::{SampleCount, ScopeCounters, ScopeModelSettings, ScopePicture, ScopeRule, render};
use scope_support::{
    assert_no_validation_errors, bars75_scene, cpu_counters, gpu_context_or_skip, picture_size, random_scene,
    rule_cases, run_scope_frame, small_settings, upload_scene,
};

/// Допуск сравнения картинки (см. описание файла).
const PICTURE_TOLERANCE: f32 = 2.0e-3;

/// Наибольшее расхождение премультиплицированных RGBA между GPU и эталоном.
fn worst_premultiplied_difference(gpu: &[[f32; 4]], cpu: &ScopePicture) -> f32 {
    assert_eq!(gpu.len(), cpu.pixels().len());
    let premultiplied = |pixel: [f32; 4]| [pixel[0] * pixel[3], pixel[1] * pixel[3], pixel[2] * pixel[3], pixel[3]];
    gpu.iter()
        .zip(cpu.pixels())
        .flat_map(|(gpu_pixel, cpu_pixel)| {
            let (gpu_premultiplied, cpu_premultiplied) = (premultiplied(*gpu_pixel), premultiplied(*cpu_pixel));
            (0..4).map(move |channel| (gpu_premultiplied[channel] - cpu_premultiplied[channel]).abs())
        })
        .fold(0.0, f32::max)
}

#[test]
fn picture_matches_cpu_render_within_fp16_tolerance_for_every_scope() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings: ScopeModelSettings = small_settings();
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    let size = picture_size(160, 96);
    let mut worst_overall = 0.0_f32;
    for scene in [bars75_scene(), random_scene(0x5EED_1234)] {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (3, 2));
        let shown = input.shown_codes(HorizontalMirror::Off);
        for (rule_name, rule) in rule_cases(&settings) {
            let frame = ScopeFrame {
                input: input.input(HorizontalMirror::Off),
                rule: &rule,
                settings: &settings,
                picture_size: size,
                frame_slot: 0,
            };
            let (recorded, _) = run_scope_frame(&context, &mut scopes, &frame).unwrap();
            assert_eq!(recorded.size, size);
            let readback = scopes.read_back_picture_blocking().unwrap();
            assert_eq!(readback.size, size);
            let counters: ScopeCounters = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
            assert_eq!(
                scopes.read_back_counters_blocking().unwrap(),
                counters,
                "{rule_name}: счётчики"
            );
            let samples = SampleCount::for_window(shown.width(), shown.height(), settings.sample_stride);
            let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
            let worst = worst_premultiplied_difference(&readback.texels, &cpu);
            eprintln!(
                "{} / {rule_name}: наибольшее расхождение картинки {worst:.2e}",
                scene.name
            );
            assert!(
                worst <= PICTURE_TOLERANCE,
                "{} / {rule_name}: расхождение {worst} больше допуска {PICTURE_TOLERANCE}",
                scene.name
            );
            worst_overall = worst_overall.max(worst);

            // Свойства картинки по формуле, а не только «GPU == CPU»: фон прозрачен, след и сетка видны.
            let transparent = readback.texels.iter().filter(|pixel| pixel[3] == 0.0).count();
            let visible = readback.texels.iter().filter(|pixel| pixel[3] > 0.0).count();
            assert!(
                transparent > readback.texels.len() / 2,
                "{rule_name}: фон прозрачен ({transparent})"
            );
            assert!(visible > 100, "{rule_name}: на картинке есть след и сетка ({visible})");
            assert!(
                readback
                    .texels
                    .iter()
                    .all(|pixel| pixel.iter().all(|channel| channel.is_finite()))
            );
        }
    }
    eprintln!("наибольшее расхождение картинки по всем приборам: {worst_overall:.2e}");
    assert_no_validation_errors(&context);
}

/// Сетка присутствует и без следа: на пустом входе (все отсчёты вне картинки) остаётся только сетка, а её
/// пиксели совпадают с эталоном; фон остаётся полностью прозрачным (альфа ровно 0).
#[test]
fn empty_trace_leaves_only_the_graticule_over_a_transparent_background() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        sample_stride: scope_model::SampleStride::try_new(64).unwrap(),
        ..small_settings()
    };
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    let scene = random_scene(7);
    let input = upload_scene(&context, &scene, StorageFormatPolicy::HalfFloatOnly, (0, 0));
    let size = picture_size(128, 128);
    for (rule_name, rule) in rule_cases(&settings) {
        let before = scopes.pass_counters();
        // Окно 20×20 при шаге 64: смещение 31 больше размера окна — отсчётов нет.
        let tiny = ScopeFrame {
            input: isp_vulkan::scopes::ScopeInput {
                view: input.image.sampled_view(),
                window: gpu_vulkan::source_window::SourceWindow::of_content(ash::vk::Extent2D {
                    width: 20,
                    height: 20,
                }),
            },
            rule: &rule,
            settings: &settings,
            picture_size: size,
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &tiny).unwrap();
        let after = scopes.pass_counters();
        assert_eq!(
            after.accumulate_dispatches, before.accumulate_dispatches,
            "{rule_name}: накопления нет"
        );
        assert_eq!(
            after.render_dispatches,
            before.render_dispatches + 1,
            "{rule_name}: показ есть"
        );
        let counters = scopes.read_back_counters_blocking().unwrap();
        assert_eq!(counters.total(), 0, "{rule_name}: отсчётов нет");
        let readback = scopes.read_back_picture_blocking().unwrap();
        // Окно 20×20 при шаге 64: отсчётов нет, опорное число — наименьшее (одно попадание).
        let samples = SampleCount::for_window(20, 20, settings.sample_stride);
        let cpu = render(&counters, &rule, &settings.render, size, samples).unwrap();
        assert!(
            worst_premultiplied_difference(&readback.texels, &cpu) <= PICTURE_TOLERANCE,
            "{rule_name}"
        );
        let grid_pixels = readback.texels.iter().filter(|pixel| pixel[3] > 0.0).count();
        assert!(grid_pixels > 50, "{rule_name}: сетка нарисована ({grid_pixels})");
        assert!(
            readback.texels.contains(&[0.0; 4]),
            "{rule_name}: фон — прозрачный ноль"
        );
        let first_visible = readback.texels.iter().find(|pixel| pixel[3] > 0.0).unwrap();
        // Только сетка: её цвет и непрозрачность из настроек показа (допуск fp16).
        assert!(
            first_visible[3] <= settings.render.graticule_opacity + PICTURE_TOLERANCE,
            "{rule_name}"
        );
    }
    assert_no_validation_errors(&context);
}
