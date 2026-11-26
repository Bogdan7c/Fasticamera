//! Яркость следа по столбцам на GPU при НЕРАВНОМЕРНОЙ раздаче отсчётов по столбцам (корень вертикальных полос RGB
//! waveform и parade): картинка GPU совпадает с эталоном `scope-model`, у которого счётчик каждого столбца приведён
//! масштабом «среднее / свои отсчёты».
//!
//! Телефон в портрете: ширина глаза 1536, шаг 2 → 768 отсчётов на 512 столбцов, то есть 256 столбцов с одним
//! отсчётом по горизонтали и 256 с двумя. Без масштаба были бы вертикальные полосы через столбец; здесь масштабы
//! берутся из таблицы блока показа по столбцам. Поворот телефона переносит горизонталь глаза на высоту
//! поверхности — проверяется отдельно.

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
use scope_model::{SampleCount, ScopeModelSettings, render, samples_in_columns};
use scope_support::{
    Scene, Xorshift, assert_no_validation_errors, gpu_context_or_skip, picture_size, rule_cases_for_scan,
    run_scope_frame, upload_scene,
};

/// Допуск сравнения картинки (тот же, что у `scope_picture.rs`).
const PICTURE_TOLERANCE: f32 = 2.0e-3;

/// Столбцов следа: столько, чтобы шаг 2 по ширине 1536 давал чередование «один / два отсчёта».
const TRACE_COLUMNS: u32 = 512;

/// Случайная картинка `width × height`: коды разбросаны по уровням, поэтому ячейки следа заполнены редко и
/// яркость лежит на крутой части лог-шкалы (там разница порогов видна).
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
        trace_columns: TRACE_COLUMNS,
        trace_levels: 128,
        vectorscope_size: 64,
        ..ScopeModelSettings::default()
    }
}

#[test]
fn picture_matches_the_cpu_reference_when_samples_split_unevenly_between_columns_for_every_scan() {
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
    let size = picture_size(1024, 96);
    // Портрет и «вверх ногами» читают по ширине, поворот на 90° — по высоте: длинная сторона глаза всегда 1536.
    for (scan, scene) in [
        (ScopeScan::Upright, random_scene("wide-1536x600", 1536, 600)),
        (ScopeScan::UpsideDown, random_scene("wide-1536x600", 1536, 600)),
        (ScopeScan::RotatedClockwise, random_scene("tall-600x1536", 600, 1536)),
        (
            ScopeScan::RotatedCounterClockwise,
            random_scene("tall-600x1536", 600, 1536),
        ),
    ] {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (4, 2));
        let shown = input.shown_codes(HorizontalMirror::Off);
        let samples = SampleCount::for_window(shown.width(), shown.height(), settings.sample_stride);
        // Предпосылка теста: отсчёты по столбцам действительно неравны (иначе тест ничего не различал бы).
        let per_column = samples_in_columns(samples, scan, TRACE_COLUMNS);
        let (fewest, most) = (per_column.iter().min().unwrap(), per_column.iter().max().unwrap());
        assert_eq!(
            *most,
            fewest * 2,
            "{scan:?}: столбцы с одним и двумя отсчётами по глазу"
        );
        // Порог кадра выше нижней границы «одно попадание» и счётчики не упираются в потолок яркости: иначе масштаб
        // столбца ничего не менял бы в картинке и тест его не различал.
        assert!(
            settings.render.trace_full_share_of_column.get() * *fewest as f32 > 2.0,
            "{scan:?}: слишком мало отсчётов в столбце"
        );
        for (rule_name, rule) in rule_cases_for_scan(&settings, scan)
            .into_iter()
            .filter(|(name, _)| matches!(*name, "waveform" | "parade"))
        {
            let frame = ScopeFrame {
                input: input.input(HorizontalMirror::Off),
                rule: &rule,
                settings: &settings,
                picture_size: size,
                frame_slot: 0,
            };
            run_scope_frame(&context, &mut scopes, &frame).unwrap();
            let counters = scopes.read_back_counters_blocking().unwrap();
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
            // Картинка не пустая: след действительно нарисован.
            assert!(
                gpu.texels.iter().filter(|pixel| pixel[3] > 0.1).count() > 1000,
                "{rule_name} / {scan:?}: след не нарисован"
            );
        }
    }
    assert_no_validation_errors(&context);
}
