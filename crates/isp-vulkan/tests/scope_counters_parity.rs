//! Паритет счётчиков приборов: GPU (RADV, полная validation) == `scope_model::accumulate` БИТ В БИТ.
//!
//! Матрица: 6 сцен (градиент, серая шкала по всем 1024 кодам, полосы 75 %, случайные коды, границы кодов, узкая
//! сцена) × 2 формата входа (10-бит UNORM и fp16) × 5 приборов (waveform, parade, вектороскоп Y'CbCr, вектороскоп
//! OKLab на sRGB и на Display P3) × зеркало и без × шаг 1, 2, 3 × окно меньше ёмкости картинки (в «хвосте»
//! картинки лежат случайные «ядовитые» пиксели).
//!
//! Эталон строится из ЧТЕНИЯ той же картинки, которую читал GPU, а отдельная проверка убеждает, что чтение
//! равно задуманным кодам (иначе «все 1024 кода» ничего бы не доказывали).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::{StorageFormatPolicy, StorageImageFormat};
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame};
use scope_model::{SampleStride, ScopeModelSettings, ScopeRule, ShownCodes};
use scope_support::{
    all_scenes, assert_no_validation_errors, cpu_counters, gpu_context_or_skip, mirror_name, picture_size, rule_cases,
    run_scope_frame, small_settings, upload_scene,
};

const FORMATS: [(StorageFormatPolicy, StorageImageFormat); 2] = [
    (
        StorageFormatPolicy::PreferTenBitUnorm,
        StorageImageFormat::A2B10G10R10Unorm,
    ),
    (StorageFormatPolicy::HalfFloatOnly, StorageImageFormat::Rgba16Float),
];

#[test]
fn counters_match_cpu_bit_exact_on_synthetic_pictures() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base_settings = small_settings();
    let rules = rule_cases(&base_settings);
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 2,
            counter_sizing: base_settings.clone(),
        },
    )
    .unwrap();
    let mut compared = 0_usize;
    let mut non_empty = 0_usize;
    for scene in all_scenes() {
        for (policy, expected_format) in FORMATS {
            let input = upload_scene(&context, &scene, policy, (7, 5));
            assert_eq!(input.image.format(), expected_format, "формат входа");
            // Чтение картинки == задуманные коды: вход действительно тот, что мы думаем.
            let intended = ShownCodes::try_new(scene.width, scene.height, scene.codes.clone()).unwrap();
            assert_eq!(
                input.shown_codes(HorizontalMirror::Off),
                intended,
                "{} {expected_format:?}",
                scene.name
            );
            for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
                let shown = input.shown_codes(mirror);
                for stride in [1_u32, 2, 3] {
                    let settings = ScopeModelSettings {
                        sample_stride: SampleStride::try_new(stride).unwrap(),
                        ..base_settings.clone()
                    };
                    for (rule_name, rule) in &rules {
                        let frame = ScopeFrame {
                            input: input.input(mirror),
                            rule,
                            settings: &settings,
                            picture_size: picture_size(160, 96),
                            frame_slot: compared % 2,
                        };
                        run_scope_frame(&context, &mut scopes, &frame).unwrap();
                        let gpu = scopes.read_back_counters_blocking().unwrap();
                        let cpu = cpu_counters(&shown, &settings, rule as &dyn ScopeRule);
                        assert_eq!(
                            gpu,
                            cpu,
                            "{} / {expected_format:?} / {rule_name} / {} / шаг {stride}: счётчики GPU и CPU разошлись",
                            scene.name,
                            mirror_name(mirror)
                        );
                        compared += 1;
                        non_empty += usize::from(cpu.total() > 0);
                    }
                }
            }
        }
    }
    // Сравнений много и большинство не пустые (иначе «равны» могло бы значить «оба нули»).
    assert_eq!(compared, 6 * 2 * 2 * 3 * 5, "размер матрицы");
    assert!(
        non_empty * 10 >= compared * 9,
        "непустых сравнений {non_empty} из {compared}"
    );
    assert_eq!(scopes.pass_counters().recorded, compared as u64);
    assert_no_validation_errors(&context);
}

/// Серая шкала по ВСЕМ 1024 кодам: на waveform — ровная диагональ (по одной ячейке на код), на вектороскопе
/// Y'CbCr и OKLab — точка в центре. Это проверка ПО ФОРМУЛЕ, а не только «GPU == CPU».
#[test]
fn gray_ramp_draws_a_diagonal_on_waveform_and_one_centre_cell_on_vectorscopes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        trace_columns: 1024,
        trace_levels: 1024,
        sample_stride: SampleStride::EVERY_PIXEL,
        ..small_settings()
    };
    let rules = rule_cases(&settings);
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    let scene = scope_support::gray_ramp_scene();
    let input = upload_scene(&context, &scene, StorageFormatPolicy::HalfFloatOnly, (0, 0));
    for (rule_name, rule) in &rules {
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule,
            settings: &settings,
            picture_size: picture_size(64, 64),
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let counters = scopes.read_back_counters_blocking().unwrap();
        let shape = counters.shape();
        match *rule_name {
            "waveform" => {
                // Столбец x ↔ уровень x: ровно 3 отсчёта (3 строки картинки) в ячейке (строка 1023 − x, столбец x).
                for code in 0..1024_u32 {
                    assert_eq!(counters.count(0, 1023 - code, code), Some(3), "код {code}");
                }
                assert_eq!(counters.total(), 3 * 1024);
            }
            "parade" => {
                // Три одинаковые плоскости (серый), каждая — та же диагональ.
                for plane in 0..3 {
                    for code in 0..1024_u32 {
                        assert_eq!(
                            counters.count(plane, 1023 - code, code),
                            Some(3),
                            "плоскость {plane} код {code}"
                        );
                    }
                }
            }
            _ => {
                let centre = shape.columns() / 2;
                assert_eq!(
                    counters.count(0, centre, centre),
                    Some(3 * 1024),
                    "{rule_name}: серый — центр"
                );
                assert_eq!(counters.total(), 3 * 1024, "{rule_name}: вне центра ничего");
            }
        }
    }
    assert_no_validation_errors(&context);
}
