//! Паритет слоя вида в КАЧЕСТВЕННОМ конвейере (V4) с эталоном `isp-model` (решения 98, 99, 176): цвет вида
//! (насыщенность, сочность, полосы), теплота, таблица (LUT) — в выходном проходе, обе основы тона, обе цели вывода.
//! Допуски — как у `quality_pipeline_parity.rs`: линейный RGB камеры бит в бит (вид его не трогает), сигнал
//! экрана — 1e-3 (шаг fp16 у 1,0 = 4,9e-4).
//!
//! Нейтральный вид здесь — бит в бит основа; вид, который меняет картинку, обязан отличаться от нейтрального
//! (иначе паритет пуст).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod look_support;
mod quality_support;

use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::look::LookControls;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::run_reference_encoded;
use isp_model::sensor::SensorDescription;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_support::real_capture::load_real_capture;
use look_contract::LookSettings;
use look_support::{
    all_bands, bars_parity_cases, controls, gray_ramp_mosaic, hue_sweep_mosaic, identity_lut, knob, known_lut,
    saturation_vibrance, wheels_parity_cases, with_lut,
};
use quality_support::{
    area_of, assert_no_validation_errors, gpu_context_or_skip, max_rgb_difference, padded_frame, run_gpu,
};

/// Сигнал экрана (fp16-цель): четверть шага 8 бит, как у прежней основы.
const DISPLAY_TOLERANCE: f32 = 1e-3;

fn params(
    sensor: &SensorDescription,
    neutral: CameraNeutral,
    gamut: DisplayGamut,
    base: ToneBase,
    look: &LookControls,
) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls = ProcessingControls {
        tone_base: base,
        look: look.clone(),
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut)
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

/// Обе основы тона: прежняя (W = цвета выхода) и AgX (W = Rec. 2020).
fn bases() -> [(&'static str, ToneBase); 2] {
    [
        ("прежняя", ToneBase::Legacy),
        ("AgX", ToneBase::Agx(AgxSettings::default())),
    ]
}

/// Наибольшее |Δ| сигнала экрана GPU против эталона одного кадра.
fn display_difference(
    context: &gpu_vulkan::context::GpuContext,
    raw: &RawMosaic,
    params: &IspParams,
) -> (f32, Vec<[f32; 3]>) {
    let (frame, frame_width, frame_height) = padded_frame(raw, 1023);
    let source = SyntheticRaw16::new(context, frame_width, frame_height, &frame).unwrap();
    let (_, display) = run_gpu(context, &source.source(), params, area_of(raw));
    let reference = run_reference_encoded(raw, params).unwrap();
    let (difference, at) = max_rgb_difference(&display, reference.pixels(), raw.size().width);
    assert!(
        difference <= DISPLAY_TOLERANCE,
        "экран: max |Δ| {difference:e} в {at:?} (допуск {DISPLAY_TOLERANCE:e})"
    );
    (difference, display)
}

/// Кадры паритета: три реальных кропа и развёртка по оттенку с калибровкой реальной камеры.
fn frames() -> Vec<(String, SensorDescription, CameraNeutral, RawMosaic)> {
    let mut frames = Vec::new();
    for name in [
        "real-main-lamp-jar",
        "real-main-lamp-towel",
        "real-main-lamp-tap-highlight",
    ] {
        let capture = load_real_capture(&isp_support::assets_dir(), name);
        frames.push((name.to_owned(), capture.sensor, capture.neutral, capture.raw));
    }
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    // Серая шкала от почти чёрного до пересвета: колёса по зонам красят и тени, и середину, и света.
    let ramp = gray_ramp_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    frames.push((
        "развёртка по оттенку".to_owned(),
        calibration.sensor.clone(),
        calibration.neutral,
        sweep,
    ));
    frames.push(("серая шкала".to_owned(), calibration.sensor, calibration.neutral, ramp));
    frames
}

#[test]
fn look_matches_reference_for_both_bases_and_outputs_and_differs_from_neutral() {
    let Some(context) = gpu_context_or_skip() else { return };
    let named_looks: Vec<(&str, LookControls)> = vec![
        ("насыщенность +1", saturation_vibrance(1.0, 0.0)),
        ("насыщенность −1", saturation_vibrance(-1.0, 0.0)),
        ("сочность +1", saturation_vibrance(0.0, 1.0)),
        ("сочность −1", saturation_vibrance(0.0, -1.0)),
        ("полосы: оттенок +1", all_bands(1.0, 0.0, 0.0)),
        ("полосы: оттенок −1", all_bands(-1.0, 0.0, 0.0)),
        ("полосы: насыщенность ±", all_bands(0.0, -1.0, 1.0)),
        ("полосы: всё сразу", all_bands(0.7, -0.6, 0.5)),
        (
            "теплота+оттенок+цвет",
            controls(LookSettings {
                warmth: knob(0.8),
                tint: knob(-0.5),
                saturation: knob(0.3),
                ..LookSettings::NEUTRAL
            }),
        ),
        ("таблица-единица", with_lut(LookControls::default(), identity_lut())),
        ("известная таблица", with_lut(LookControls::default(), known_lut())),
        (
            "известная таблица + цвет",
            with_lut(saturation_vibrance(0.5, 0.3), known_lut()),
        ),
    ];
    let mut looks: Vec<(String, LookControls)> = named_looks
        .into_iter()
        .map(|(name, look)| (name.to_owned(), look))
        .collect();
    looks.extend(wheels_parity_cases());
    looks.extend(bars_parity_cases());
    for (frame_name, sensor, neutral, raw) in frames() {
        for (base_name, base) in bases() {
            for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
                let plain = params(&sensor, neutral, gamut, base, &LookControls::default());
                let (_, plain_display) = display_difference(&context, &raw, &plain);
                for (look_name, look) in &looks {
                    let looked = params(&sensor, neutral, gamut, base, look);
                    let (difference, display) = display_difference(&context, &raw, &looked);
                    let moved = display
                        .iter()
                        .zip(&plain_display)
                        .flat_map(|(left, right)| left.iter().zip(right).map(|(a, b)| (a - b).abs()))
                        .fold(0.0_f32, f32::max);
                    println!(
                        "{frame_name} / {base_name} / {gamut:?} / {look_name}: |Δ| против эталона {difference:.2e}, \
                         сдвиг против нейтрали {moved:.3}"
                    );
                    // «Единица» не двигает картинку заметно; остальные виды — двигают (паритет не пуст) хотя бы
                    // на одной из сцен: у реальных кропов с малой цветностью сдвиг может быть мал, поэтому порог
                    // проверяется на развёртке по оттенку ниже.
                    // Колёса красят зоны светлоты: их сдвиг проверяется на серой шкале, где есть и тени, и света.
                    let is_wheels_look = look_name.starts_with("колесо")
                        || look_name.starts_with("четыре колеса")
                        || look_name.starts_with("полоски");
                    let checked_frame = if is_wheels_look {
                        "серая шкала"
                    } else {
                        "развёртка по оттенку"
                    };
                    if frame_name == checked_frame && look_name != "таблица-единица" {
                        assert!(
                            moved > 0.01,
                            "{look_name} / {base_name}: вид не двигает картинку ({moved})"
                        );
                    }
                }
            }
        }
    }
    assert_no_validation_errors(&context);
}

/// Ручки, явно заданные в нуль, разрешаются в «вида нет» (стадии пропущены) и дают ту же картинку, что вид по
/// умолчанию; вид с ручками картинку меняет. Побитовое равенство нейтрали основе держит общая функция
/// `look_apply_color` (ранний возврат по флагу) — её ломает тест видоискателя
/// `neutral_look_is_bit_identical_to_the_base_...`, где сравнение идёт с набором стадий БЕЗ вида; выходной проход
/// качественного пути и снимка зовёт ту же функцию.
#[test]
fn explicitly_neutral_look_resolves_to_no_stages_and_a_touched_look_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (_, sensor, neutral, raw) = frames().pop().unwrap();
    for (base_name, base) in bases() {
        let plain = params(&sensor, neutral, DisplayGamut::Srgb, base, &LookControls::default());
        let explicit = params(
            &sensor,
            neutral,
            DisplayGamut::Srgb,
            base,
            &controls(LookSettings::NEUTRAL),
        );
        assert!(explicit.look.color.is_none() && explicit.look.lut.is_none());
        let (_, first) = display_difference(&context, &raw, &plain);
        let (_, second) = display_difference(&context, &raw, &explicit);
        assert_eq!(
            first, second,
            "{base_name}: явная нейтраль разошлась с видом по умолчанию"
        );
        let touched = params(
            &sensor,
            neutral,
            DisplayGamut::Srgb,
            base,
            &saturation_vibrance(0.9, 0.0),
        );
        let (_, looked) = display_difference(&context, &raw, &touched);
        assert_ne!(first, looked, "{base_name}: вид не изменил картинку");
    }
    assert_no_validation_errors(&context);
}
