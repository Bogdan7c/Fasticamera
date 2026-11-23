//! Новый прибор добавляется БЕЗ правки существующих: «подставной» прибор — гистограмма в одну строку со своим
//! шейдером (`test_scope_histogram.comp` на общей части `scope_accumulate_common.glsl`) и своим правилом
//! `GpuScopeRule`, зарегистрированный при создании. Его счётчики на GPU равны тому же правилу `ScopeRule` на CPU
//! бит в бит; картинку показа ему рисует тот же проход, что и настоящим приборам.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{
    AccumulateParameters, AccumulatorId, AccumulatorShader, ColorScopes, ColorScopesConfig, GpuScopeRule, RuleWords,
    ScopeError, ScopeFrame,
};
use isp_vulkan::shaders::blobs;
use scope_model::{
    GraticulePrimitive, GridShape, Hit, MAX_GRATICULE_PRIMITIVES, RenderSettings, SamplePosition, SampleStride,
    ScopeModelSettings, ScopeRule,
};
use scope_support::{
    all_scenes, assert_no_validation_errors, cpu_counters, gpu_context_or_skip, picture_size, rule_cases,
    run_scope_frame, small_settings, upload_scene,
};

const HISTOGRAM_BINS: u32 = 1024;
const HISTOGRAM_COLOR: [f32; 3] = [0.2, 0.4, 0.6];

fn histogram_id() -> AccumulatorId {
    AccumulatorId::new("test-histogram")
}

/// Подставной прибор: столбец = `(R + G + B) / divisor`, одна строка, одна плоскость.
struct FakeHistogram {
    divisor: u32,
}

impl ScopeRule for FakeHistogram {
    fn shape(&self) -> GridShape {
        GridShape::try_new(HISTOGRAM_BINS, 1, 1).unwrap()
    }

    fn hits(&self, _position: SamplePosition, code: [u16; 3], report: &mut dyn FnMut(Hit)) {
        let sum = u32::from(code[0]) + u32::from(code[1]) + u32::from(code[2]);
        report(Hit {
            plane: 0,
            row: 0,
            column: sum / self.divisor,
        });
    }
}

impl GpuScopeRule for FakeHistogram {
    fn accumulator(&self) -> AccumulatorId {
        histogram_id()
    }

    fn grid(&self) -> GridShape {
        self.shape()
    }

    fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError> {
        let mut words = RuleWords::new();
        words.push_u32(self.divisor);
        Ok(AccumulateParameters {
            words,
            decode_table: None,
        })
    }

    fn trace_colors(&self, _settings: &RenderSettings) -> Vec<[f32; 3]> {
        vec![HISTOGRAM_COLOR]
    }

    fn graticule(&self, _settings: &RenderSettings) -> Vec<GraticulePrimitive> {
        Vec::new()
    }
}

fn histogram_shader() -> AccumulatorShader {
    AccumulatorShader {
        id: histogram_id(),
        spirv: blobs::TEST_SCOPE_HISTOGRAM_COMP,
    }
}

fn scopes_with_histogram(context: &gpu_vulkan::context::GpuContext, settings: &ScopeModelSettings) -> ColorScopes {
    ColorScopes::with_extra_accumulators(
        context,
        &ColorScopesConfig {
            frame_slots: 2,
            counter_sizing: settings.clone(),
        },
        &[histogram_shader()],
    )
    .unwrap()
}

#[test]
fn fake_kind_counts_on_gpu_equal_to_the_same_rule_on_cpu_and_coexists_with_production_kinds() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base_settings = small_settings();
    let mut scopes = scopes_with_histogram(&context, &base_settings);
    assert_eq!(scopes.accumulator_pipeline_count(), 3, "два стандартных + подставной");
    let live_pipelines = context.live_objects().count(LiveObjectKind::Pipeline);
    let production = rule_cases(&base_settings);
    let mut compared = 0;
    for scene in all_scenes() {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (4, 3));
        for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
            let shown = input.shown_codes(mirror);
            for stride in [1_u32, 2] {
                let settings = ScopeModelSettings {
                    sample_stride: SampleStride::try_new(stride).unwrap(),
                    ..base_settings.clone()
                };
                for divisor in [3_u32, 4] {
                    let fake = FakeHistogram { divisor };
                    let frame = ScopeFrame {
                        input: input.input(mirror),
                        rule: &fake,
                        settings: &settings,
                        picture_size: picture_size(160, 96),
                        frame_slot: compared % 2,
                    };
                    run_scope_frame(&context, &mut scopes, &frame).unwrap();
                    let gpu = scopes.read_back_counters_blocking().unwrap();
                    assert_eq!(
                        gpu,
                        cpu_counters(&shown, &settings, &fake),
                        "{} делитель {divisor}",
                        scene.name
                    );
                    assert!(gpu.total() > 0);
                    compared += 1;
                    // После подставного прибора настоящий в том же владельце считает по-прежнему верно.
                    let (name, real) = &production[compared % production.len()];
                    let real_frame = ScopeFrame {
                        rule: real,
                        frame_slot: compared % 2,
                        ..frame
                    };
                    run_scope_frame(&context, &mut scopes, &real_frame).unwrap();
                    assert_eq!(
                        scopes.read_back_counters_blocking().unwrap(),
                        cpu_counters(&shown, &settings, real as &dyn ScopeRule),
                        "{name} после подставного"
                    );
                }
            }
        }
    }
    assert_eq!(
        context.live_objects().count(LiveObjectKind::Pipeline),
        live_pipelines,
        "пайплайны не создавались"
    );
    assert_no_validation_errors(&context);
}

/// Картинка подставного прибора: пиксель следа есть ровно там, где счётчик столбца не нулевой, и несёт цвет прибора.
#[test]
fn fake_kind_picture_shows_trace_exactly_where_counters_are_nonzero() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let mut scopes = scopes_with_histogram(&context, &settings);
    let input = upload_scene(
        &context,
        &scene_with_few_levels(),
        StorageFormatPolicy::HalfFloatOnly,
        (0, 0),
    );
    let fake = FakeHistogram { divisor: 3 };
    let size = picture_size(256, 8);
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: &fake,
        settings: &ScopeModelSettings {
            sample_stride: SampleStride::EVERY_PIXEL,
            ..settings.clone()
        },
        picture_size: size,
        frame_slot: 0,
    };
    run_scope_frame(&context, &mut scopes, &frame).unwrap();
    let counters = scopes.read_back_counters_blocking().unwrap();
    let picture = scopes.read_back_picture_blocking().unwrap();
    let (mut with_trace, mut without_trace) = (0, 0);
    for y in 0..size.height() {
        for x in 0..size.width() {
            let column = x * HISTOGRAM_BINS / size.width();
            let count = counters.count(0, 0, column).unwrap();
            let pixel = picture.texels[(y * size.width() + x) as usize];
            if count > 0 {
                with_trace += 1;
                assert!(pixel[3] > 0.1, "({x},{y}): след есть, альфа {}", pixel[3]);
                for channel in 0..3 {
                    assert!(
                        (pixel[channel] - HISTOGRAM_COLOR[channel]).abs() < 2.0e-3,
                        "({x},{y}) цвет прибора"
                    );
                }
            } else {
                without_trace += 1;
                assert_eq!(pixel, [0.0; 4], "({x},{y}): следа нет — прозрачный ноль");
            }
        }
    }
    assert!(with_trace > 0 && without_trace > 0, "{with_trace} / {without_trace}");
    assert_no_validation_errors(&context);
}

fn scene_with_few_levels() -> scope_support::Scene {
    scope_support::Scene {
        name: "few-levels",
        width: 16,
        height: 4,
        codes: (0..64).map(|index| [[0u16, 300, 600, 1023][index % 4]; 3]).collect(),
    }
}

/// Отказы нового прибора — те же, что у настоящих, и ДО команд.
#[test]
fn fake_kind_errors_are_reported_before_any_command() {
    let Some(context) = gpu_context_or_skip() else { return };
    let settings = small_settings();
    let input = upload_scene(
        &context,
        &scene_with_few_levels(),
        StorageFormatPolicy::HalfFloatOnly,
        (0, 0),
    );

    // Шейдер не зарегистрирован: владелец без подставного шейдера отказывает.
    let mut plain = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 1,
            counter_sizing: settings.clone(),
        },
    )
    .unwrap();
    let fake = FakeHistogram { divisor: 3 };
    let frame = ScopeFrame {
        input: input.input(HorizontalMirror::Off),
        rule: &fake,
        settings: &settings,
        picture_size: picture_size(64, 32),
        frame_slot: 0,
    };
    assert!(matches!(
        run_scope_frame(&context, &mut plain, &frame),
        Err(ScopeError::UnknownAccumulator { id }) if id == histogram_id()
    ));
    assert_eq!(plain.pass_counters().recorded, 0);

    // Повторная регистрация того же идентификатора — отказ при создании.
    let duplicate = ColorScopes::with_extra_accumulators(
        &context,
        &ColorScopesConfig::default(),
        &[histogram_shader(), histogram_shader()],
    );
    assert!(matches!(duplicate, Err(ScopeError::DuplicateAccumulator { .. })));

    // Прибор с неверным числом цветов, слишком большим числом слов и примитивов.
    struct Broken {
        colors: usize,
        words: usize,
        primitives: usize,
    }
    impl GpuScopeRule for Broken {
        fn accumulator(&self) -> AccumulatorId {
            histogram_id()
        }
        fn grid(&self) -> GridShape {
            GridShape::try_new(HISTOGRAM_BINS, 1, 1).unwrap()
        }
        fn accumulate_parameters(&self) -> Result<AccumulateParameters<'_>, ScopeError> {
            let mut words = RuleWords::new();
            for _ in 0..self.words {
                words.push_u32(3);
            }
            Ok(AccumulateParameters {
                words,
                decode_table: None,
            })
        }
        fn trace_colors(&self, _settings: &RenderSettings) -> Vec<[f32; 3]> {
            vec![[1.0; 3]; self.colors]
        }
        fn graticule(&self, _settings: &RenderSettings) -> Vec<GraticulePrimitive> {
            vec![
                GraticulePrimitive::Segment {
                    from: [0.0; 2],
                    to: [1.0; 2],
                    half_width_px: 1.0
                };
                self.primitives
            ]
        }
    }
    let mut scopes = scopes_with_histogram(&context, &settings);
    let attempt = |scopes: &mut ColorScopes, broken: &Broken| {
        let frame = ScopeFrame {
            input: input.input(HorizontalMirror::Off),
            rule: broken,
            settings: &settings,
            picture_size: picture_size(64, 32),
            frame_slot: 0,
        };
        scopes
            .prepare_picture(frame.picture_size)
            .expect("картинка подготовлена");
        let mut outcome = None;
        run_one_shot_blocking(&context, |command_buffer| {
            outcome = Some(scopes.record(command_buffer, &frame).map(|_| ()));
            Ok(())
        })
        .unwrap();
        outcome.unwrap()
    };
    let valid = Broken {
        colors: 1,
        words: 1,
        primitives: 1,
    };
    attempt(&mut scopes, &valid).unwrap();
    assert_eq!(
        attempt(&mut scopes, &Broken { colors: 2, ..valid }),
        Err(ScopeError::TraceColorCountMismatch { colors: 2, planes: 1 })
    );
    assert!(matches!(
        attempt(&mut scopes, &Broken { words: 61, ..valid }),
        Err(ScopeError::TooManyRuleWords { words: 61, .. })
    ));
    assert_eq!(
        attempt(
            &mut scopes,
            &Broken {
                primitives: MAX_GRATICULE_PRIMITIVES + 1,
                ..valid
            }
        ),
        Err(ScopeError::TooManyGraticulePrimitives {
            count: MAX_GRATICULE_PRIMITIVES + 1,
            max: MAX_GRATICULE_PRIMITIVES
        })
    );
    // Принятый кадр записан один, отказы ничего не добавили.
    assert_eq!(scopes.pass_counters().recorded, 1);
    assert_no_validation_errors(&context);
}
