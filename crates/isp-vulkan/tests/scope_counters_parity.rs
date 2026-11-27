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
use scope_model::{ResolvedScope, SampleStride, ScopeModelSettings, ScopeRule, ShownCodes};
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
                        // Шаг гистограммы — своя настройка: матрица проходит и её шаги.
                        histogram_sample_stride: SampleStride::try_new(stride).unwrap(),
                        cie: scope_model::CieSettings {
                            sample_stride: SampleStride::try_new(stride).unwrap(),
                            ..base_settings.cie.clone()
                        },
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
    assert_eq!(compared, 6 * 2 * 2 * 3 * rules.len(), "размер матрицы");
    assert_eq!(
        rules.len(),
        13,
        "waveform, parade, два вектороскопа (три гаммы), Luma, три режима гистограммы и CIE (две шкалы × два охвата)"
    );
    assert!(
        non_empty * 10 >= compared * 9,
        "непустых сравнений {non_empty} из {compared}"
    );
    assert_eq!(scopes.pass_counters().recorded, compared as u64);
    assert_no_validation_errors(&context);
}

/// Серая шкала по ВСЕМ 1024 кодам: на waveform и parade — ровная диагональ в каждой из трёх плоскостей (по одной ячейке на код), на вектороскопе
/// Y'CbCr и OKLab — точка в центре. Это проверка ПО ФОРМУЛЕ, а не только «GPU == CPU».
#[test]
fn gray_ramp_draws_a_diagonal_on_waveform_and_one_centre_cell_on_vectorscopes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        trace_max_columns: 1024,
        trace_levels: 1024,
        sample_stride: SampleStride::EVERY_PIXEL,
        // Все 1024 кода — и у вектороскопа (его шаг по умолчанию 2, решение 328), и у следов поперёк взгляда (329).
        vectorscope_sample_stride: SampleStride::EVERY_PIXEL,
        trace_across_eye_stride: SampleStride::EVERY_PIXEL,
        histogram_sample_stride: SampleStride::EVERY_PIXEL,
        cie: scope_model::CieSettings {
            sample_stride: SampleStride::EVERY_PIXEL,
            ..small_settings().cie
        },
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
            "waveform" | "parade" => {
                // RGB waveform и parade — одно правило: три одинаковые плоскости (серый), каждая — та же диагональ
                // (столбец x ↔ уровень x: ровно 3 отсчёта по числу строк картинки в ячейке (1023 − x, x)).
                for plane in 0..3 {
                    for code in 0..1024_u32 {
                        assert_eq!(
                            counters.count(plane, 1023 - code, code),
                            Some(3),
                            "плоскость {plane} код {code}"
                        );
                    }
                }
                assert_eq!(
                    counters.total(),
                    3 * 3 * 1024,
                    "{rule_name}: по три плоскости на отсчёт"
                );
            }
            "luma" => {
                // Яркость серого — сам код: та же диагональ, но одна плоскость (решение 321).
                assert_eq!(shape.planes(), 1);
                for code in 0..1024_u32 {
                    assert_eq!(counters.count(0, 1023 - code, code), Some(3), "яркость, код {code}");
                }
                assert_eq!(counters.total(), 3 * 1024, "{rule_name}: по одному попаданию на отсчёт");
            }
            "histogram-rgb" | "histogram-parade" | "histogram-luma" => {
                // Гистограмма: 1024 кода по 3 строки на 512 уровней — по 2 кода × 3 строки = 6 отсчётов на уровень каждой
                // плоскости (столбец — уровень кода, строка одна).
                let planes = if *rule_name == "histogram-luma" { 1 } else { 3 };
                assert_eq!((shape.rows(), shape.planes()), (1, planes), "{rule_name}: форма");
                for plane in 0..planes {
                    for bin in 0..shape.columns() {
                        assert_eq!(
                            counters.count(plane, 0, bin),
                            Some(6),
                            "{rule_name}: плоскость {plane}, уровень {bin}"
                        );
                    }
                }
                assert_eq!(
                    counters.total(),
                    u64::from(planes) * 3 * 1024,
                    "{rule_name}: по попаданию на плоскость"
                );
            }
            name if name.starts_with("cie-") => {
                // Серая шкала лежит у точки белого: каждый код выше порога яркости даёт 3 попадания (три строки)
                // в ячейку не дальше двух клеток от ячейки белого; тёмные коды (ниже порога) не дают ничего.
                let ResolvedScope::Cie(cie) = rule else {
                    panic!("{name}: не CIE")
                };
                let white = cie.cell_of_code([1023; 3]).expect("белый считается");
                let counted = (0..1024_u16)
                    .filter(|code| cie.cell_of_code([*code; 3]).is_some())
                    .count() as u64;
                assert!(
                    counted > 900,
                    "{name}: порог яркости съел слишком много кодов ({counted})"
                );
                assert_eq!(counters.total(), 3 * counted, "{name}: по три отсчёта на считаемый код");
                for row in 0..shape.rows() {
                    for column in 0..shape.columns() {
                        if counters.count(0, row, column).unwrap_or(0) > 0 {
                            assert!(
                                column.abs_diff(white[0]) <= 2 && row.abs_diff(white[1]) <= 2,
                                "{name}: серый вне окрестности белого: ({column}, {row}), белый {white:?}"
                            );
                        }
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

/// Окно шире потолка столбцов: план отсчётов (`SampleCount::planned`) сам берёт бо́льший шаг, и GPU делает то же, что
/// эталон, — бит в бит. Случайная сцена 97 × 61 при потолке 40 столбцов: шаг 1 дал бы 97, шаг 2 — 49 (оба выше
/// потолка), шаг 3 — 32 столбца (x = 1, 4, …, 94: смещение шага 1).
#[test]
fn a_window_wider_than_the_column_ceiling_raises_the_stride_identically_on_gpu_and_cpu() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = ScopeModelSettings {
        trace_max_columns: 40,
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
    let scene = scope_support::random_scene(0x7357_0001);
    assert_eq!(
        (scene.width, scene.height),
        (97, 61),
        "предпосылка теста — размер сцены"
    );
    let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (2, 2));
    let shown = input.shown_codes(HorizontalMirror::Off);
    for (rule_name, rule) in rule_cases(&settings) {
        // Пол шага под потолок — 3 по ОБЕИМ осям (у следа поперёк взгляда просили 2 — пол сильнее, решение 329).
        let samples = scope_support::planned_samples_for(&shown, &settings, &rule);
        assert_eq!(
            (samples.steps().across().get(), samples.steps().down().get()),
            (3, 3),
            "{rule_name}"
        );
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: &rule,
            settings: &settings,
            picture_size: picture_size(120, 64),
            frame_slot: 0,
        };
        run_scope_frame(&context, &mut scopes, &frame).unwrap();
        let gpu = scopes.read_back_counters_blocking().unwrap();
        let cpu = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
        assert_eq!(gpu, cpu, "{rule_name}: счётчики GPU и CPU разошлись");
        assert!(cpu.total() > 0, "{rule_name}: счётчики не пусты");
        if matches!(rule_name, "waveform" | "luma" | "parade") {
            assert_eq!(gpu.shape().columns(), 32, "{rule_name}: столбец на отсчёт при шаге 3");
        }
    }
    assert_no_validation_errors(&context);
}
