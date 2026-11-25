//! Сила ручек тона вида на GPU в видоискателе (решения 242–246) и поведение серого экрана при подтяжке AE.
//!
//! * Порог силы: серый клин (256 столбцов, −9…0 ступеней относительно белого сенсора) идёт через ВЕСЬ слой видоискателя
//!   на GPU; каждая ручка тона на краю на КАЖДОЙ из четырёх основ двигает код экрана не меньше порога владельца
//!   (контраст ±46, средние ±112, тени +59/−23, света +20/−58 кодов из 255) и в правильную сторону. Мера — по readback
//!   fp16-выхода, как у эталонного стенда `isp-model/tests/look_knob_strength.rs`.
//! * Серый и AE: при подтяжке AE ±0,5 EV серый сцены (подтянутый к серому экрана) после тона остаётся на сером экрана для
//!   контраста, теней и светов (их кривая держит серый), а средние тона его двигают — для этого ручка и есть.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::params::ExposureGain;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::filmic::FilmicSettings;
use isp_vulkan::viewfinder::ViewfinderStorageFormat;
use look_support::tone_look;
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, assert_no_validation_errors, gpu_context_or_skip, run_viewfinder, test_config,
};

/// Масштаб кода экрана.
const CODE_SCALE: f32 = 255.0;
/// Ширина и высота RAW-клина. Выход видоискателя вдвое меньше (одна ячейка Байера — один пиксель): 128 столбцов дают шаг
/// 9/127 ≈ 0,07 ступени — пик ручки не прячется между столбцами.
const WEDGE_WIDTH: u32 = 256;
const WEDGE_HEIGHT: u32 = 8;
/// Самый тёмный столбец клина, ступени относительно белого сенсора (верхний столбец — сам белый).
const WEDGE_DARKEST_STOPS: f64 = -9.0;
/// Средний серый экрана линейно (умолчание всех основ и стадии тона).
const DISPLAY_MIDDLE_GRAY: f32 = 0.20;
/// Серый сцены моделей по умолчанию (одинаков у AgX, ACES, filmic и прежней).
const SCENE_MIDDLE_GRAY: f64 = 0.075;

fn four_bases() -> [(&'static str, ToneBase); 4] {
    [
        ("AgX", ToneBase::Agx(AgxSettings::default())),
        ("ACES", ToneBase::Aces(AcesSettings::default())),
        ("filmic", ToneBase::Filmic(FilmicSettings::default())),
        ("прежняя", ToneBase::Legacy),
    ]
}

fn spec(base: ToneBase) -> ParamsSpec {
    ParamsSpec {
        gamut: DisplayGamut::DisplayP3,
        tone_base: base,
        ..ParamsSpec::default()
    }
}

/// Серый клин: яркость растёт слева направо от `WEDGE_DARKEST_STOPS` до белого сенсора (логарифмически), серый под нейтралью.
fn gray_wedge(spec: &ParamsSpec) -> RawFrame {
    let brightest = spec.neutral.iter().copied().fold(f64::MIN, f64::max);
    RawFrame::from_linear(
        WEDGE_WIDTH,
        WEDGE_HEIGHT,
        spec.pattern,
        spec.levels(),
        |x, _, channel| {
            let stops = WEDGE_DARKEST_STOPS * (1.0 - f64::from(x) / f64::from(WEDGE_WIDTH - 1));
            stops.exp2() * spec.neutral[channel] / brightest
        },
    )
}

/// Равномерный серый кадр с линейным уровнем сенсора `level`.
fn gray_patch(spec: &ParamsSpec, level: f64) -> RawFrame {
    let brightest = spec.neutral.iter().copied().fold(f64::MIN, f64::max);
    RawFrame::from_linear(
        WEDGE_WIDTH,
        WEDGE_HEIGHT,
        spec.pattern,
        spec.levels(),
        |_, _, channel| level * spec.neutral[channel] / brightest,
    )
}

/// Код экрана `0…255` (среднее по каналам) в столбце `column` средней строки кадра GPU (fp16).
fn code_at(run: &viewfinder_support::ViewfinderRun, column: u32) -> f32 {
    let pixel = run.pixel(column, run.height / 2);
    CODE_SCALE * (pixel[0] + pixel[1] + pixel[2]) / 3.0
}

/// Код экрана в центре кадра (равномерный серый).
fn center_code(run: &viewfinder_support::ViewfinderRun) -> f32 {
    code_at(run, run.width / 2)
}

/// Прогон кадра на GPU в fp16 (кодировка выхода — OETF Display P3).
fn gpu_run(
    context: &gpu_vulkan::context::GpuContext,
    spec: &ParamsSpec,
    raw: &RawFrame,
    params: &isp_model::params::IspParams,
) -> viewfinder_support::ViewfinderRun {
    run_viewfinder(
        context,
        &test_config(ViewfinderStorageFormat::HalfFloatOnly),
        &RunRequest::whole_frame(raw, params, spec.display()),
    )
}

/// Наибольшие сдвиги кода клина вверх и вниз против нейтрали и столбцы, где они случились.
struct WedgeShift {
    up_codes: f32,
    up_column: u32,
    down_codes: f32,
    down_column: u32,
}

fn wedge_shift(neutral: &viewfinder_support::ViewfinderRun, touched: &viewfinder_support::ViewfinderRun) -> WedgeShift {
    let mut shift = WedgeShift {
        up_codes: 0.0,
        up_column: 0,
        down_codes: 0.0,
        down_column: 0,
    };
    assert_eq!(neutral.width, touched.width);
    for column in 0..neutral.width {
        let change = code_at(touched, column) - code_at(neutral, column);
        if change > shift.up_codes {
            (shift.up_codes, shift.up_column) = (change, column);
        }
        if -change > shift.down_codes {
            (shift.down_codes, shift.down_column) = (-change, column);
        }
    }
    shift
}

/// Ручка тона: имя, значения ручек на плюс и минус края и порог владельца `(минус, плюс)` в кодах из 255.
struct ToneKnobThreshold {
    name: &'static str,
    /// Значения (контраст, средние, тени, света) при ходе ручки `+1`.
    plus_edge: [f32; 4],
    minus_codes: f32,
    plus_codes: f32,
}

const TONE_KNOB_THRESHOLDS: [ToneKnobThreshold; 4] = [
    ToneKnobThreshold {
        name: "КОНТРАСТ",
        plus_edge: [1.0, 0.0, 0.0, 0.0],
        minus_codes: 46.0,
        plus_codes: 46.0,
    },
    ToneKnobThreshold {
        name: "СРЕДНИЕ",
        plus_edge: [0.0, 1.0, 0.0, 0.0],
        minus_codes: 112.0,
        plus_codes: 112.0,
    },
    ToneKnobThreshold {
        name: "ТЕНИ",
        plus_edge: [0.0, 0.0, 1.0, 0.0],
        minus_codes: 23.0,
        plus_codes: 59.0,
    },
    ToneKnobThreshold {
        name: "СВЕТА",
        plus_edge: [0.0, 0.0, 0.0, 1.0],
        minus_codes: 58.0,
        plus_codes: 20.0,
    },
];

#[test]
fn every_tone_knob_at_its_edge_moves_the_gpu_gray_wedge_by_at_least_the_owner_threshold_on_every_base() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut failures = Vec::new();
    for (base_name, base) in four_bases() {
        let spec = spec(base);
        let wedge = gray_wedge(&spec);
        let neutral = gpu_run(&context, &spec, &wedge, &spec.build());
        for knob in &TONE_KNOB_THRESHOLDS {
            for (sign, threshold) in [(1.0_f32, knob.plus_codes), (-1.0, knob.minus_codes)] {
                let [contrast, midtones, shadows, highlights] = knob.plus_edge.map(|value| value * sign);
                let params = spec.build_with_look(tone_look(contrast, midtones, shadows, highlights));
                let shift = wedge_shift(&neutral, &gpu_run(&context, &spec, &wedge, &params));
                // Плюс светлит (сдвиг вверх), минус темнит (сдвиг вниз): главное направление и величина.
                let (moved, column, opposite) = if sign > 0.0 {
                    (shift.up_codes, shift.up_column, shift.down_codes)
                } else {
                    (shift.down_codes, shift.down_column, shift.up_codes)
                };
                eprintln!(
                    "{base_name} {} {sign:+}: {moved:.1} кодов в столбце {column} (порог {threshold}); обратно {opposite:.1}",
                    knob.name
                );
                if moved < threshold {
                    failures.push(format!(
                        "{base_name} {} {sign:+}: сдвиг {moved:.1} < порога {threshold} (столбец {column})",
                        knob.name
                    ));
                }
                // Знак ручки не перепутан: в обратную сторону она заметно не тянет (контраст — исключение:
                // он светлит света и темнит тени одновременно, поэтому и вверх, и вниз).
                if knob.name != "КОНТРАСТ" && opposite > moved {
                    failures.push(format!(
                        "{base_name} {} {sign:+}: в обратную сторону {opposite:.1} больше, чем в свою {moved:.1}",
                        knob.name
                    ));
                }
            }
        }
    }
    assert_no_validation_errors(&context);
    assert!(failures.is_empty(), "сила ручек на GPU:\n  {}", failures.join("\n  "));
}

#[test]
fn ae_edges_keep_the_scene_gray_on_the_display_gray_except_for_midtones() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut failures = Vec::new();
    // Серый экрана 0,20 в коде экрана: ожидаемый код нейтрали (OETF Display P3 = sRGB).
    let encode = |linear: f32| {
        if linear <= 0.003_130_8 {
            12.92 * linear
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        }
    };
    let gray_code = CODE_SCALE * encode(DISPLAY_MIDDLE_GRAY);
    for (base_name, base) in four_bases() {
        let spec = spec(base);
        for exposure_ev in [-0.5_f32, 0.5] {
            let gain = ExposureGain::from_ev(exposure_ev).unwrap();
            // Серый сцены при подтяжке AE: кривая основы делит серый на подтяжку, поэтому на серый экрана падает
            // уровень `серый / подтяжка` (при +0,5 EV AE уже подтянул кадр: нужен темнее).
            let level = SCENE_MIDDLE_GRAY / 2.0_f64.powf(f64::from(exposure_ev));
            let patch = gray_patch(&spec, level);
            let neutral_params = spec.build_with_look_and_exposure(LookControls::default(), gain);
            let neutral_code = center_code(&gpu_run(&context, &spec, &patch, &neutral_params));
            eprintln!("{base_name} AE {exposure_ev:+}: нейтраль {neutral_code:.2} кода, серый экрана {gray_code:.2}");
            assert!(
                (neutral_code - gray_code).abs() < 1.5,
                "{base_name} AE {exposure_ev:+}: серый сцены не лёг на серый экрана ({neutral_code} против {gray_code}): \
                 уровень пятна подобран неверно"
            );
            for (knob_name, edges) in [
                ("КОНТРАСТ", [1.0, 0.0, 0.0, 0.0]),
                ("ТЕНИ", [0.0, 0.0, 1.0, 0.0]),
                ("СВЕТА", [0.0, 0.0, 0.0, 1.0]),
                ("СРЕДНИЕ", [0.0, 1.0, 0.0, 0.0]),
            ] {
                for sign in [1.0_f32, -1.0] {
                    let [contrast, midtones, shadows, highlights] = edges.map(|value| value * sign);
                    let params =
                        spec.build_with_look_and_exposure(tone_look(contrast, midtones, shadows, highlights), gain);
                    let code = center_code(&gpu_run(&context, &spec, &patch, &params));
                    let shift = code - neutral_code;
                    eprintln!(
                        "{base_name} AE {exposure_ev:+} {knob_name} {sign:+}: серый сдвинулся на {shift:+.2} кода"
                    );
                    if knob_name == "СРЕДНИЕ" {
                        // Средние тона для того и нужны: серый уходит заметно и в свою сторону.
                        if shift * sign < 20.0 {
                            failures.push(format!(
                                "{base_name} AE {exposure_ev:+} СРЕДНИЕ {sign:+}: серый сдвинулся лишь на {shift:+.2}"
                            ));
                        }
                    } else if shift.abs() > GRAY_HOLD_CODES {
                        failures.push(format!(
                            "{base_name} AE {exposure_ev:+} {knob_name} {sign:+}: серый ушёл на {shift:+.2} кода \
                             (допустимо ±{GRAY_HOLD_CODES})"
                        ));
                    }
                }
            }
        }
    }
    assert_no_validation_errors(&context);
    assert!(failures.is_empty(), "серый экрана:\n  {}", failures.join("\n  "));
}

/// Насколько серый может сдвинуться от контраста, теней и светов (коды из 255): серый держит сама кривая (склейка
/// половин в `q = √0,20` с равными наклонами), остаются округления fp16 и 8-битная мера и расхождение серого основы с 0,20.
const GRAY_HOLD_CODES: f32 = 1.0;
