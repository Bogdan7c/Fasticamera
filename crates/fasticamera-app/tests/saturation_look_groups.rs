//! Группы стенда насыщения `look_cost`, `look_parity` и `look_parity_primaries` на настоящем GPU ПК (RADV, полная validation) через раннер.
//!
//! Что проверяется (результат, а не вызов):
//! * `look_cost`: у КАЖДОЙ пары «вкл против выкл» картинка выхода отличается (рычаг действительно меняет работу GPU, а
//!   не мерит пустоту), в итоге варианта есть отчёт `diff_vs=<эталон>` с ненулевым отличием, и ни одна работа не
//!   компилировала пайплайн за время прогона (иначе цена включала бы остановку на компиляцию);
//! * `look_parity`: линейка паритета на ПК даёт у каждой из 16 работ числа; на нейтрали «лампа» ошибка математики не
//!   больше стандартного допуска 1e-5 у всех, а у видов с включённой веткой она больше нуля (измерение происходит, а
//!   не «пустой нулевой отчёт»); на нейтрали D65 каждая работа проходит под своим допуском (полосы — именованный).
//!
//! Печатает таблицы (`-- --nocapture`): мс на кадр по работам цены и худшая ошибка по работам паритета на RADV.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

use std::time::Duration;

use camera_contract::lens_shading::LensShadingCalibrations;
use fasticamera::camera_screen_config::CameraScreenConfig;
use fasticamera::look_service::LookHandle;
use fasticamera::look_startup::catalog_environment;
use fasticamera::measurement_look::MeasurementLookConfig;
use fasticamera::power_measurement::ThermalSnapshot;
use fasticamera::saturation_bench::catalog::{CatalogEntry, DisplayScreen, WorkloadGroup, find_entry, group_entries};
use fasticamera::saturation_bench::compare_pairs::PAIRS;
use fasticamera::saturation_bench::digest_report::DigestOutcome;
use fasticamera::saturation_bench::gpu_executor::GpuWorkloadExecutor;
use fasticamera::saturation_bench::inputs::{InputSources, ProductInputs, gather_inputs};
use fasticamera::saturation_bench::parity_report::{ParityOutcome, ParityRecord};
use fasticamera::saturation_bench::report::{RowOutcome, RunSummary, WorkloadRow};
use fasticamera::saturation_bench::runner::{BenchRunner, PauseOutcome, RunClock, RunEvent, ThermalProbe};
use fasticamera::saturation_bench::screen_scene::{ScreenPlan, SurfaceSizePx};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::saturation::{CancelToken, SaturationConfig};
use gpu_vulkan::swapchain::FramesInFlight;
use gpu_vulkan::validation::ValidationMode;
use isp_model::color::dng::CameraNeutral;
use look_contract::{GlobalKnob, LookIntent, LookKnob};
use look_presets::PresetStoreConfig;
use render_loop::config::RenderLoopConfig;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Допуск ошибки математики сверх шага хранения (`ErrorAllowance::Standard`).
const MATH_EXCESS_TOLERANCE: f32 = 1.0e-5;

/// Нейтраль кадра «лампа» (основная камера, 4532 K): та, на которой установлены поля допуска паритета 14a и 14i
/// (`isp-vulkan/tests/viewfinder_support`: `LAMP`).
const LAMP_NEUTRAL: [f64; 3] = [0.515625, 1.0, 0.4990234375];

/// Входы продукта без камеры: запасной сенсор и библиотека пресетов в памяти. Вид продукта делается НЕнейтральным
/// (контраст, насыщенность и сочность тронуты умеренно, как у «Как глаз» владельца), иначе пара «продукт против нейтрали» на умолчании библиотеки
/// не различалась бы по определению.
fn product_inputs() -> ProductInputs {
    let render = RenderLoopConfig::default();
    let look = LookHandle::in_memory(&PresetStoreConfig::default(), &catalog_environment(&render)).expect("библиотека");
    for (knob, value) in [
        (GlobalKnob::Contrast, 0.02),
        (GlobalKnob::Saturation, 0.08),
        (GlobalKnob::Vibrance, 0.10),
    ] {
        look.apply(LookIntent::SetKnob {
            knob,
            value: LookKnob::new(value).expect("ручка в диапазоне"),
        })
        .expect("намерение принято");
    }
    let screen = CameraScreenConfig::default();
    let lens_shading = LensShadingCalibrations::none();
    gather_inputs(&InputSources {
        engine_status: None,
        lens_shading: &lens_shading,
        render_config: &render,
        look: Some(&look),
        screen: &screen,
    })
    .with_measurement_look(MeasurementLookConfig::default())
}

struct NoPause;

impl RunClock for NoPause {
    fn elapsed(&self) -> Duration {
        Duration::ZERO
    }
    fn pause(&self, _duration: Duration, _cancel: &CancelToken) -> PauseOutcome {
        PauseOutcome::Elapsed
    }
}

struct NoThermal;

impl ThermalProbe for NoThermal {
    fn snapshot(&self) -> ThermalSnapshot {
        ThermalSnapshot {
            status: None,
            headroom: None,
            error: Some("на ПК теплового сервиса нет".to_owned()),
        }
    }
}

/// Прогон работ через раннер стенда (окно короткое: нужны картинки и числа паритета, а не точная цена).
fn run_entries(context: &GpuContext, inputs: ProductInputs, entries: &[&'static CatalogEntry]) -> RunSummary {
    let bench = SaturationConfig {
        frames_in_flight: FramesInFlight::new(2).unwrap(),
        warmup: Duration::from_millis(100),
        measure: Duration::from_millis(300),
        timeline_step: Duration::from_millis(100),
        fence_timeout: Duration::from_secs(30),
    };
    let mut executor = GpuWorkloadExecutor::new(context.clone(), inputs, bench, SurfaceSizePx::ONEPLUS_11);
    let mut observer = |_event: RunEvent| {};
    let summary = BenchRunner {
        executor: &mut executor,
        clock: &NoPause,
        thermal: &NoThermal,
        observer: &mut observer,
        cooldown_between: Duration::ZERO,
    }
    .run(entries, &CancelToken::new());
    drop(executor);
    summary
}

fn row<'a>(summary: &'a RunSummary, name: &str) -> &'a WorkloadRow {
    summary.rows.iter().find(|row| row.name == name).expect("строка работы")
}

/// Отпечаток части `image` выхода строки; паника с причиной, если его нет.
fn image_digest(summary: &RunSummary, name: &str) -> String {
    match row(summary, name)
        .digest
        .as_ref()
        .unwrap_or_else(|| panic!("{name}: нет дайджеста"))
    {
        DigestOutcome::Computed { parts, .. } => parts
            .iter()
            .find(|part| part.label == "image")
            .unwrap_or_else(|| panic!("{name}: нет части image"))
            .digest
            .clone(),
        DigestOutcome::Unavailable { reason } => panic!("{name}: дайджест недоступен: {reason}"),
    }
}

/// Заметка о кадре (`frame_note`) строки: там отчёт `diff_vs=`.
fn frame_note(summary: &RunSummary, name: &str) -> String {
    match row(summary, name).digest.as_ref().expect("дайджест") {
        DigestOutcome::Computed { frame_note, .. } => frame_note.clone().unwrap_or_default(),
        DigestOutcome::Unavailable { reason } => panic!("{name}: дайджест недоступен: {reason}"),
    }
}

fn gpu_context_or_skip() -> Option<GpuContext> {
    match GpuContext::new(&GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL)) {
        Ok(context) => Some(context),
        Err(error) if std::env::var(SKIP_ENV).as_deref() == Ok("1") => {
            eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
            None
        }
        Err(error) => panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно"),
    }
}

#[test]
fn every_look_cost_lever_changes_the_picture_and_no_work_compiles_a_pipeline_during_the_run() {
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::LookCost);
    let summary = run_entries(&context, product_inputs(), &entries);
    assert_eq!(summary.rows.len(), entries.len());

    println!("| работа | мс/кадр | создано пайплайнов | image |");
    for entry in &entries {
        let one = row(&summary, entry.name);
        let RowOutcome::Measured { measurement } = &one.outcome else {
            panic!("{}: нет цифры: {:?}", entry.name, one.outcome);
        };
        println!(
            "| {} | {:.3} | {:?} | {} |",
            entry.name,
            measurement.frame_cost_ms.expect("цена кадра"),
            one.extras.pipelines_created_during_run,
            image_digest(&summary, entry.name)
        );
        assert_eq!(
            one.extras.pipelines_created_during_run,
            Some(0),
            "{}: пайплайн создан посреди прогона (компиляция вариантов не должна попадать в окно замера)",
            entry.name
        );
    }

    let look_pairs: Vec<_> = PAIRS.iter().filter(|pair| pair.variant.starts_with("look_")).collect();
    assert_eq!(look_pairs.len(), 7);
    for pair in look_pairs {
        assert_ne!(
            image_digest(&summary, pair.variant),
            image_digest(&summary, pair.reference),
            "{} и {} дали одну картинку: рычаг не дошёл до GPU, цена была бы ценой пустоты",
            pair.variant,
            pair.reference
        );
        let note = frame_note(&summary, pair.variant);
        assert!(
            note.contains(&format!("diff_vs={}", pair.reference)) && !note.contains("сравнить нельзя"),
            "{}: в итоге нет отчёта об отличии от эталона: {note}",
            pair.variant
        );
    }
}

/// Входы паритета на нейтрали «лампа»: условия, в которых поля допуска 14a и 14i установлены.
fn lamp_inputs() -> ProductInputs {
    let mut inputs = product_inputs();
    inputs.neutral = CameraNeutral::new(LAMP_NEUTRAL).expect("нейтраль лампы положительна");
    inputs
}

/// Запись паритета строки; паника с причиной, если числа нет.
fn parity_record<'a>(summary: &'a RunSummary, name: &str) -> &'a ParityRecord {
    let one = row(summary, name);
    assert!(one.is_measured(), "{name}: нет цифры цены: {:?}", one.outcome);
    match &one.extras.parity {
        Some(ParityOutcome::Measured(record)) => record,
        other => panic!("{name}: паритет не измерен: {other:?}"),
    }
}

#[test]
fn look_parity_works_measure_on_the_pc_gpu_and_stay_inside_the_allowance_on_the_lamp_neutral() {
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::LookParity);
    let summary = run_entries(&context, lamp_inputs(), &entries);
    assert_eq!(summary.rows.len(), 16);

    println!("| работа | худшая ошибка | max |Δ| | сцена | формат 10 бит |");
    let mut worst_of_branches = 0.0_f32;
    for entry in &entries {
        let record = parity_record(&summary, entry.name);
        println!(
            "| {} | {:.3e} | {:.3e} | {} | {} |",
            entry.name, record.worst_math_excess, record.worst_max_abs, record.worst_scene, record.preferred_format
        );
        assert_eq!(record.scenes_measured, 7, "{}: измерены не все сцены", entry.name);
        assert_eq!(record.preferred_format, "rgb10a2", "RADV пишет rgb10a2");
        assert!(
            record.worst_math_excess <= MATH_EXCESS_TOLERANCE && record.passed(),
            "{}: ошибка математики {:.3e} > допуск {MATH_EXCESS_TOLERANCE:e}: {:?}",
            entry.name,
            record.worst_math_excess,
            record.violations
        );
        assert_eq!(
            row(&summary, entry.name).violation(),
            None,
            "{}: строка без нарушений",
            entry.name
        );
        // Измерение настоящее: GPU не бит в бит с эталоном (округление хранения), max |Δ| больше нуля.
        assert!(
            record.worst_max_abs > 0.0,
            "{}: max |Δ| = 0 — линейка ничего не мерила",
            entry.name
        );
        if !entry.name.ends_with("_product") {
            worst_of_branches = worst_of_branches.max(record.worst_math_excess);
        }
    }
    // Хоть одна ветка (полосы, колёса, тон) даёт ошибку математики выше нуля: измеряется шейдер, а не пустота.
    assert!(
        worst_of_branches > 0.0,
        "ошибка математики у всех видов с веткой ровно ноль: измерение не доходит до шейдера?"
    );
}

/// Нейтраль по умолчанию стенда (D65 профиля сенсора, как у продукта) — та, на которой 14o нашёл превышение 1e-5 у
/// полос оттенка (AgX, filmic: 1,35e-5 на fp16, на RADV и Adreno одинаково). Причина доказана опытом: вес «не серый»
/// раздувает ULP-разницу основы у почти серых пикселей, поэтому у рецептов с тронутыми полосами свой именованный
/// допуск `near_neutral_bands_amplification` (3e-5). Тест проверяет, что стенд на этой нейтрали ЗЕЛЁНЫЙ под честными
/// допусками: полосы — под именованным, всё остальное — под стандартным 1e-5 (превышение там — дефект).
#[test]
fn on_the_default_d65_neutral_every_parity_work_passes_under_its_own_named_allowance() {
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::LookParity);
    let summary = run_entries(&context, product_inputs(), &entries);
    println!("| работа | худшая ошибка (D65) | допуск | имя допуска |");
    for entry in &entries {
        let record = parity_record(&summary, entry.name);
        println!(
            "| {} | {:.3e} | {:e} | {} |",
            entry.name, record.worst_math_excess, record.tolerance, record.allowance
        );
        let expected_allowance = if entry.name.ends_with("_bands") {
            "near_neutral_bands_amplification"
        } else {
            "standard"
        };
        assert_eq!(record.allowance, expected_allowance, "{}: не тот допуск", entry.name);
        assert!(
            record.passed(),
            "{}: ошибка математики {:.3e} > допуск {:e}: {:?}",
            entry.name,
            record.worst_math_excess,
            record.tolerance,
            record.violations
        );
        assert_eq!(
            row(&summary, entry.name).violation(),
            None,
            "{}: строка без нарушений",
            entry.name
        );
    }
}

/// 14t: паритет стадии полосок на ПК. Образец 12 полосок на четырёх основах: на лампе и на D65 ошибка математики
/// внутри СТАНДАРТНОГО допуска (полоски не получают именованного послабления), измерение настоящее (ошибка больше
/// нуля: шейдер полосок исполнялся и сравнился с CPU-формулой), все сцены измерены.
#[test]
fn primaries_parity_works_measure_the_primaries_shader_and_pass_the_standard_allowance_on_both_neutrals() {
    let Some(context) = gpu_context_or_skip() else { return };
    let entries = group_entries(WorkloadGroup::LookParityPrimaries);
    assert_eq!(entries.len(), 4);
    for (neutral_name, inputs) in [("лампа", lamp_inputs()), ("D65", product_inputs())] {
        let summary = run_entries(&context, inputs, &entries);
        println!("| работа ({neutral_name}) | худшая ошибка | max |Δ| | допуск |");
        for entry in &entries {
            let record = parity_record(&summary, entry.name);
            println!(
                "| {} | {:.3e} | {:.3e} | {} |",
                entry.name, record.worst_math_excess, record.worst_max_abs, record.allowance
            );
            assert_eq!(
                record.allowance, "standard",
                "{}: полоскам именованный допуск не положен",
                entry.name
            );
            assert_eq!(record.scenes_measured, 7, "{}: измерены не все сцены", entry.name);
            assert!(
                record.passed() && record.worst_math_excess <= MATH_EXCESS_TOLERANCE,
                "{} ({neutral_name}): ошибка математики {:.3e} > допуск {MATH_EXCESS_TOLERANCE:e}: {:?}",
                entry.name,
                record.worst_math_excess,
                record.violations
            );
            assert!(
                record.worst_max_abs > 0.0,
                "{}: max |Δ| = 0 — линейка ничего не мерила",
                entry.name
            );
        }
    }
}

/// Пункт 5 сессии 14o: работа `scopes_*` называет в итоге размер картинки, на котором мерила, и это размер раздела
/// CALIBRATE по настоящей раскладке экрана (форма вектороскопа — квадрат, вписанный в ту же зону).
#[test]
fn scopes_works_report_the_section_picture_size_from_the_real_calibrate_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let inputs = product_inputs();
    let plan = ScreenPlan::build(&inputs, SurfaceSizePx::ONEPLUS_11).expect("списки рисования");
    let zone = plan
        .scene(DisplayScreen::Calibrate)
        .scope_request
        .expect("раздел просит прибор")
        .image_size;
    let names = ["scopes_waveform", "scopes_parade", "scopes_vectorscope_ycbcr"];
    let entries: Vec<&'static CatalogEntry> = names.iter().map(|name| find_entry(name).expect("работа")).collect();
    let summary = run_entries(&context, inputs, &entries);
    for name in names {
        let picture = row(&summary, name)
            .extras
            .scope_picture
            .unwrap_or_else(|| panic!("{name}: размер не назван"));
        let expected = if name.contains("vectorscope") {
            let square = zone.width().min(zone.height());
            (square, square)
        } else {
            (zone.width(), zone.height())
        };
        println!("{name}: {}", picture.compact());
        assert_eq!((picture.width, picture.height), expected, "{name}");
        assert!(picture.width > 0 && picture.height > 0, "{name}");
    }
    // Сводка называет размеры строками `имя|ШxВ`.
    let rows = fasticamera::saturation_bench::summary::scope_picture_rows(&summary);
    assert_eq!(rows.len(), 3);
    assert!(
        rows[0].starts_with("scopes_waveform|") && !rows[0].ends_with("|-"),
        "{rows:?}"
    );
}
