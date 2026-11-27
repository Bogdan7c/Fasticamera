//! Диаграмма CIE на GPU (сессия 14s): счётчики == эталон БИТ В БИТ (обе шкалы, оба охвата, четыре положения телефона,
//! зеркало, синтетика, граничные коды, RAW-подобная сцена), картинка == эталон в допуске fp16 при каждом цвете следа и
//! заливке, и точное деление: поправка по произведениям держит ответ при приближении, сдвинутом на целую клетку.
//!
//! Число пайплайнов `ColorScopes::new` проверяется отдельным тестом: каждый стоит 145–180 мс на телефоне при открытии раздела.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_model::image::DisplayGamut;
use isp_vulkan::scopes::{
    AccumulateParameters, AccumulatorId, ColorScopes, ColorScopesConfig, GpuScopeRule, ScopeError, ScopeFrame,
    cie_guess_skew_diagnostics,
};
use scope_contract::{CieDiagram, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{
    CieLocusFill, CieSettings, CieTraceColour, ResolvedScope, SampleStride, ScopeModelSettings, ScopeRule, render,
};
use scope_support::{
    Scene, Xorshift, all_scenes, assert_no_validation_errors, cpu_counters, gpu_context_or_skip, mirror_name,
    picture_size, run_scope_frame, small_settings, upload_scene, worst_premultiplied_difference,
};

/// Допуск картинки: четыре шага fp16 у единицы (как у остальных приборов).
const PICTURE_TOLERANCE: f32 = 2.0e-3;
const DIAGRAMS: [CieDiagram; 2] = [CieDiagram::Xy1931, CieDiagram::UvPrime1976];
const GAMUTS: [DisplayGamut; 2] = [DisplayGamut::Srgb, DisplayGamut::DisplayP3];
const SCANS: [ScopeScan; 4] = [
    ScopeScan::Upright,
    ScopeScan::RotatedClockwise,
    ScopeScan::UpsideDown,
    ScopeScan::RotatedCounterClockwise,
];

fn settings_with_cie() -> ScopeModelSettings {
    ScopeModelSettings {
        // Размер не степень двойки: деление и переход в клетки проверяются на «неудобном» числе.
        cie: CieSettings {
            size: 200,
            ..CieSettings::default()
        },
        ..small_settings()
    }
}

fn cie_rule(settings: &ScopeModelSettings, diagram: CieDiagram, gamut: DisplayGamut, scan: ScopeScan) -> ResolvedScope {
    ResolvedScope::resolve(
        &ScopeRequest {
            kind: ScopeKind::Cie,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
            cie_diagram: diagram,
            histogram_mode: scope_contract::HistogramMode::default(),
            scan,
            image_size: picture_size(160, 96),
        },
        settings,
        gamut,
    )
    .unwrap()
}

fn scopes_with_diagnostics(context: &gpu_vulkan::context::GpuContext, settings: &ScopeModelSettings) -> ColorScopes {
    ColorScopes::with_extra_accumulators(
        context,
        &ColorScopesConfig {
            frame_slots: 2,
            counter_sizing: settings.clone(),
        },
        &cie_guess_skew_diagnostics(),
    )
    .unwrap()
}

/// Сцена из кодов, чья позиция в сетке (f64) лежит в 2·10⁻⁴ клетки от границы ячейки по любой оси: у них деление
/// «на грани», и приближение GPU легко ошиблось бы на клетку.
fn boundary_heavy_scene(settings: &ScopeModelSettings, diagram: CieDiagram, gamut: DisplayGamut) -> Scene {
    let rule = cie_rule(settings, diagram, gamut, ScopeScan::Upright);
    let ResolvedScope::Cie(cie) = &rule else {
        panic!("ожидалась CIE")
    };
    let projection_rows = cie.rows();
    let table = cie.decode_table();
    let (left, top, per_unit) = (
        f64::from(cie.window().left),
        f64::from(cie.window_top()),
        f64::from(cie.cells_per_unit()),
    );
    let mut random = Xorshift(0xB0DA_C1E5);
    let mut codes = Vec::new();
    let mut attempts = 0_u32;
    while codes.len() < 40 * 30 && attempts < 40_000_000 {
        attempts += 1;
        let code = [0, 1, 2].map(|_| (random.next() % 1024) as u16);
        let linear = code.map(|channel| f64::from(table.linear_light(channel)));
        let dot = |row: [f32; 3]| (0..3).map(|i| f64::from(row[i]) * linear[i]).sum::<f64>();
        let denominator = dot(projection_rows.denominator);
        if denominator <= 0.0 || dot(projection_rows.luminance) < f64::from(cie.min_luminance()) {
            continue;
        }
        let column = (dot(projection_rows.column_numerator) / denominator - left) * per_unit;
        let row = (top - dot(projection_rows.row_numerator) / denominator) * per_unit;
        let near_boundary = |value: f64| (value - value.round()).abs() < 2.0e-4;
        if near_boundary(column) || near_boundary(row) {
            codes.push(code);
        }
    }
    assert!(
        codes.len() >= 400,
        "граничных кодов найдено {} из {attempts} попыток",
        codes.len()
    );
    codes.resize(40 * 30, [900, 900, 900]);
    Scene {
        name: "cie-boundary-heavy",
        width: 40,
        height: 30,
        codes,
    }
}

/// Правило другого шейдера накопления: тот же прибор, но счёт ведёт диагностический шейдер.
struct Rerouted<'a> {
    inner: &'a ResolvedScope,
    accumulator: AccumulatorId,
}

impl GpuScopeRule for Rerouted<'_> {
    fn accumulator(&self) -> AccumulatorId {
        self.accumulator
    }
    fn grid(&self, samples: scope_model::SampleCount) -> scope_model::GridShape {
        GpuScopeRule::grid(self.inner, samples)
    }
    fn wanted_steps(&self, settings: &ScopeModelSettings) -> scope_model::SampleSteps {
        GpuScopeRule::wanted_steps(self.inner, settings)
    }
    fn scan(&self) -> ScopeScan {
        GpuScopeRule::scan(self.inner)
    }
    fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError> {
        GpuScopeRule::accumulate_parameters(self.inner)
    }
    fn trace_style(&self, settings: &scope_model::RenderSettings) -> scope_model::TraceStyle {
        GpuScopeRule::trace_style(self.inner, settings)
    }
    fn graticule(&self, settings: &scope_model::RenderSettings) -> Vec<scope_model::GraticulePrimitive> {
        GpuScopeRule::graticule(self.inner, settings)
    }
}

#[test]
fn counters_match_cpu_bit_exact_for_both_diagrams_gamuts_scans_mirrors_and_boundary_codes() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base = settings_with_cie();
    let mut scopes = scopes_with_diagnostics(&context, &base);
    let mut compared = 0_usize;
    let mut non_empty = 0_usize;
    for diagram in DIAGRAMS {
        for gamut in GAMUTS {
            let mut scenes = all_scenes();
            scenes.push(boundary_heavy_scene(&base, diagram, gamut));
            for scene in &scenes {
                let input = upload_scene(&context, scene, StorageFormatPolicy::HalfFloatOnly, (7, 5));
                for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
                    let shown = input.shown_codes(mirror);
                    for stride in [1_u32, 2] {
                        let settings = ScopeModelSettings {
                            cie: CieSettings {
                                sample_stride: SampleStride::try_new(stride).unwrap(),
                                ..base.cie.clone()
                            },
                            ..base.clone()
                        };
                        let mut first_scan_counters = None;
                        for scan in SCANS {
                            let rule = cie_rule(&settings, diagram, gamut, scan);
                            let frame = ScopeFrame {
                                input: input.input(mirror),
                                rule: &rule,
                                settings: &settings,
                                picture_size: picture_size(160, 96),
                                frame_slot: compared % 2,
                            };
                            run_scope_frame(&context, &mut scopes, &frame).unwrap();
                            let gpu = scopes.read_back_counters_blocking().unwrap();
                            let cpu = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
                            let label = format!(
                                "{diagram:?} {gamut:?} {} {} шаг {stride} {scan:?}",
                                scene.name,
                                mirror_name(mirror)
                            );
                            assert_eq!(gpu, cpu, "{label}: счётчики GPU и CPU разошлись");
                            // CIE не зависит от того, как глаз читает картинку.
                            match &first_scan_counters {
                                None => first_scan_counters = Some(gpu),
                                Some(first) => {
                                    assert_eq!(&gpu, first, "{label}: счётчики зависят от положения телефона")
                                }
                            }
                            compared += 1;
                            non_empty += usize::from(cpu.total() > 0);
                        }
                    }
                }
            }
        }
    }
    assert!(
        non_empty * 10 >= compared * 8,
        "непустых сравнений {non_empty} из {compared}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn a_skewed_division_guess_still_gives_the_exact_cell_so_the_correction_not_the_driver_decides() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = settings_with_cie();
    let mut scopes = scopes_with_diagnostics(&context, &settings);
    let mut compared = 0;
    for diagram in DIAGRAMS {
        for gamut in GAMUTS {
            let rule = cie_rule(&settings, diagram, gamut, ScopeScan::Upright);
            let mut scenes = vec![
                scope_support::random_scene(0x5EED_0001),
                scope_support::gradient_scene(),
            ];
            scenes.push(boundary_heavy_scene(&settings, diagram, gamut));
            for scene in &scenes {
                let input = upload_scene(&context, scene, StorageFormatPolicy::HalfFloatOnly, (0, 0));
                let shown = input.shown_codes(HorizontalMirror::Off);
                let cpu = cpu_counters(&shown, &settings, &rule as &dyn ScopeRule);
                assert!(cpu.total() > 0);
                for id in ["cie-guess-high", "cie-guess-low"] {
                    let rerouted = Rerouted {
                        inner: &rule,
                        accumulator: AccumulatorId::new(id),
                    };
                    let frame = ScopeFrame {
                        input: input.input(HorizontalMirror::Off),
                        rule: &rerouted,
                        settings: &settings,
                        picture_size: picture_size(160, 96),
                        frame_slot: 0,
                    };
                    run_scope_frame(&context, &mut scopes, &frame).unwrap();
                    assert_eq!(
                        scopes.read_back_counters_blocking().unwrap(),
                        cpu,
                        "{id} {diagram:?} {gamut:?} {}: сдвинутое приближение изменило ответ",
                        scene.name
                    );
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 2 * 2 * 3 * 2);
    assert_no_validation_errors(&context);
}

#[test]
fn picture_matches_the_cpu_reference_for_every_trace_colour_and_locus_fill_in_both_diagrams() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base = settings_with_cie();
    let mut scopes = scopes_with_diagnostics(&context, &base);
    let scene = scope_support::bars75_scene();
    let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (3, 2));
    let shown = input.shown_codes(HorizontalMirror::Off);
    let mut worst_overall = 0.0_f32;
    for diagram in DIAGRAMS {
        for gamut in GAMUTS {
            for trace_colour in [CieTraceColour::White, CieTraceColour::ByChromaticity] {
                for locus_fill in [CieLocusFill::Outline, CieLocusFill::FaintColour] {
                    for size in [picture_size(194, 194), picture_size(333, 333)] {
                        let mut settings = base.clone();
                        settings.render.cie.trace_colour = trace_colour;
                        settings.render.cie.locus_fill = locus_fill;
                        let rule = cie_rule(&settings, diagram, gamut, ScopeScan::Upright);
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
                        let readback = scopes.read_back_picture_blocking().unwrap();
                        let worst = worst_premultiplied_difference(&readback.texels, &cpu);
                        let label = format!("{diagram:?} {gamut:?} {trace_colour:?} {locus_fill:?} {}", size.width());
                        assert!(worst <= PICTURE_TOLERANCE, "{label}: расхождение {worst}");
                        // Картинка живая: есть след и (при заливке) цветная заливка вне следа.
                        let visible = readback.texels.iter().filter(|pixel| pixel[3] > 0.0).count();
                        assert!(visible > 100, "{label}: на картинке {visible} пикселей");
                        if locus_fill == CieLocusFill::FaintColour {
                            let faint = readback
                                .texels
                                .iter()
                                .filter(|pixel| pixel[3] > 0.1 && pixel[3] < 0.3)
                                .count();
                            assert!(faint > 1000, "{label}: заливки {faint} пикселей");
                        }
                        worst_overall = worst_overall.max(worst);
                    }
                }
            }
        }
    }
    eprintln!("CIE: наибольшее расхождение картинки GPU и CPU {worst_overall:.2e}");
    assert_no_validation_errors(&context);
}

#[test]
fn color_scopes_creates_the_documented_number_of_pipelines() {
    let Some(context) = gpu_context_or_skip() else { return };
    let scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: ScopeModelSettings::default(),
        },
    )
    .unwrap();
    // Накопление: 5 обычных (integer-runs, integer, oklab, histogram, cie) + 4 со слиянием, если устройство умеет баллот.
    let merged = usize::from(context.capabilities().subgroup_ballot_in_compute) * 4;
    assert_eq!(scopes.accumulator_pipeline_count(), 5 + merged);
    // Всего с переносом (1) и показом (3: продукт, гистограмма, CIE).
    eprintln!(
        "конвейеров ColorScopes::new: {}",
        scopes.accumulator_pipeline_count() + 1 + 3
    );
}
