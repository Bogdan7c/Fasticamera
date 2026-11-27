//! Паритет слоя вида на GPU (RADV, validation) в ВИДОИСКАТЕЛЕ с эталоном `isp-model` (решения 98, 99, 176):
//! цвет вида (насыщенность, сочность, полосы оттенка), теплота, таблица (LUT), обе основы тона, обе цели вывода.
//! Допуски — действующие: max |Δ| ≤ ½ LSB 10-битного выхода + 1e-5, ноль пикселей хуже 1 LSB (`assert_viewfinder_parity`).
//!
//! Что проверяется сверх паритета:
//! * колёса цвета по зонам (Lift, Gamma, Gain, Offset): каждое отдельно, все вместе с цветом и полосой, края колец;
//! * вид действительно меняет картинку (иначе совпадение с эталоном ничего бы не доказывало);
//! * нейтральный вид — бит в бит основа (стадия пропущена флагом, а не «нулевой» формулой);
//! * переключение «нейтрален ↔ не нейтрален» НЕ меняет вариант pipeline (первое касание ручки не создаёт pipeline);
//! * таблица: «единица» не меняет картинку, известная таблица совпадает с эталоном, таблица больше буфера — явная
//!   ошибка кадра, ISP остаётся рабочим; стадии цвета и таблицы вырезаются выключателями набора.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use camera_contract::geometry::SensorOrientation;
use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_vulkan::look::LookLutLimits;
use isp_vulkan::viewfinder::{IspStage, IspStageSet, ViewfinderError, ViewfinderIsp, ViewfinderStorageFormat};
use look_contract::{LookSettings, WheelZone};
use look_support::{
    all_bands, all_wheels, band, bands, controls, identity_lut, knob, known_lut, saturation_vibrance,
    single_wheel_look, wheel, wheels_look, wheels_parity_cases, with_lut,
};
use viewfinder_support::{
    ErrorAllowance, ParamsSpec, RunRequest, assert_no_validation_errors, assert_viewfinder_parity,
    assert_viewfinder_parity_allowing, gpu_context_or_skip, hue_sweep, look_scenes, reference_upright_encoded,
    run_viewfinder, synthetic_scenes, test_config,
};

fn spec(base: ToneBase, gamut: DisplayGamut) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: base,
        ..ParamsSpec::default()
    }
}

/// Обе основы тона: прежняя (W = цвета выхода) и AgX (W = Rec. 2020).
fn bases() -> [(&'static str, ToneBase); 2] {
    [
        ("прежняя", ToneBase::Legacy),
        ("AgX", ToneBase::Agx(AgxSettings::default())),
    ]
}

/// Паритет вида `look` на всех сценах обеих основ и обеих целей вывода; возвращает худший max |Δ| fp16.
fn assert_look_parity(context: &gpu_vulkan::context::GpuContext, name: &str, look: &LookControls) -> f32 {
    let mut worst = 0.0_f32;
    for (base_name, base) in bases() {
        for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
            let allowance = if base == ToneBase::Legacy && gamut == DisplayGamut::Srgb {
                ErrorAllowance::LegacySrgbExtremeColour
            } else {
                ErrorAllowance::Standard
            };
            let spec = spec(base, gamut);
            let params = spec.build_with_look(look.clone());
            for (scene, raw) in look_scenes(&spec) {
                let (half, _) = assert_viewfinder_parity_allowing(
                    context,
                    &format!("вид {name} / {base_name} / {gamut:?} / {scene}"),
                    &raw,
                    &params,
                    &spec,
                    allowance,
                );
                worst = worst.max(half);
            }
        }
    }
    worst
}

/// Вид меняет картинку против нейтрального (по эталону): иначе совпадение GPU с эталоном — пустое.
fn assert_look_changes_the_picture(name: &str, look: &LookControls) {
    for (base_name, base) in bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let neutral = spec.build();
        let looked = spec.build_with_look(look.clone());
        let raw = hue_sweep(&spec);
        let plain = reference_upright_encoded(&raw, raw.full_rect(), SensorOrientation::Degrees0, &neutral);
        let changed = reference_upright_encoded(&raw, raw.full_rect(), SensorOrientation::Degrees0, &looked);
        let biggest = plain
            .pixels
            .iter()
            .zip(&changed.pixels)
            .flat_map(|(left, right)| left.iter().zip(right).map(|(a, b)| (a - b).abs()))
            .fold(0.0_f32, f32::max);
        assert!(
            biggest > 0.01,
            "вид {name} ({base_name}) почти не меняет картинку ({biggest}): паритет ничего не доказывает"
        );
    }
}

#[test]
fn saturation_and_vibrance_at_extremes_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    for (name, saturation, vibrance) in [
        ("насыщенность +1", 1.0, 0.0),
        ("насыщенность −1", -1.0, 0.0),
        ("сочность +1", 0.0, 1.0),
        ("сочность −1", 0.0, -1.0),
        ("обе +0,6/−0,4", 0.6, -0.4),
    ] {
        let look = saturation_vibrance(saturation, vibrance);
        assert_look_changes_the_picture(name, &look);
        let worst = assert_look_parity(&context, name, &look);
        eprintln!("{name}: худший max |Δ| fp16 = {worst:.3e}");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn every_band_knob_at_extremes_matches_reference_including_the_hue_wrap() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Все полосы сразу: сдвиг оттенка, насыщенность и яркость на обоих краях (развёртка проходит стык 360° → 0°).
    for (name, look) in [
        ("полосы: оттенок +1", all_bands(1.0, 0.0, 0.0)),
        ("полосы: оттенок −1", all_bands(-1.0, 0.0, 0.0)),
        ("полосы: насыщенность +1", all_bands(0.0, 1.0, 0.0)),
        ("полосы: насыщенность −1", all_bands(0.0, -1.0, 0.0)),
        ("полосы: яркость +1", all_bands(0.0, 0.0, 1.0)),
        ("полосы: яркость −1", all_bands(0.0, 0.0, -1.0)),
        ("полосы: всё сразу", all_bands(0.7, -0.6, 0.5)),
    ] {
        assert_look_changes_the_picture(name, &look);
        let worst = assert_look_parity(&context, name, &look);
        eprintln!("{name}: худший max |Δ| fp16 = {worst:.3e}");
    }
    // Каждая полоса по отдельности (первая и последняя соседствуют через стык круга), три ручки в крайних значениях.
    let spec = spec(ToneBase::Agx(AgxSettings::default()), DisplayGamut::DisplayP3);
    let raw = hue_sweep(&spec);
    for index in 0..look_support::model().bands.len() {
        for (hue, saturation, luminance) in [(1.0, 1.0, 1.0), (-1.0, -1.0, -1.0)] {
            let params = spec.build_with_look(bands(vec![band(index, hue, saturation, luminance)]));
            assert_viewfinder_parity(
                &context,
                &format!("полоса {index} ({hue}, {saturation}, {luminance})"),
                &raw,
                &params,
                &spec,
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn colour_knobs_together_with_warmth_and_tint_match_reference_and_move_the_white_point() {
    let Some(context) = gpu_context_or_skip() else { return };
    // Теплота и оттенок уже внутри матриц/усилений параметров кадра: GPU получает сдвинутые числа и совпадает с
    // эталоном, у которого те же числа; картинка при этом теплее (R/B выросло) против нейтрального вида.
    let warm = controls(LookSettings {
        warmth: knob(0.8),
        tint: knob(-0.5),
        saturation: knob(0.3),
        bands: look_support::all_bands(0.3, 0.2, 0.0).settings.bands.clone(),
        ..LookSettings::NEUTRAL
    });
    assert_look_changes_the_picture("теплота+оттенок+цвет", &warm);
    let worst = assert_look_parity(&context, "теплота+оттенок+цвет", &warm);
    eprintln!("теплота+оттенок+цвет: худший max |Δ| fp16 = {worst:.3e}");

    let spec = spec(ToneBase::Agx(AgxSettings::default()), DisplayGamut::DisplayP3);
    let gray = &synthetic_scenes(&spec)[0].1;
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let neutral_params = spec.build();
    let warm_params = spec.build_with_look(controls(LookSettings {
        warmth: knob(0.8),
        ..LookSettings::NEUTRAL
    }));
    assert_ne!(
        neutral_params.white_balance, warm_params.white_balance,
        "теплота сдвигает усиления баланса, которые получит GPU"
    );
    let neutral_run = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(gray, &neutral_params, spec.display()),
    );
    let warm_run = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(gray, &warm_params, spec.display()),
    );
    // Середина серой шкалы по яркости (у белого AgX сжимает любой оттенок).
    let middle = |run: &viewfinder_support::ViewfinderRun| run.pixel(run.width / 3, run.height / 2);
    let (neutral_pixel, warm_pixel) = (middle(&neutral_run), middle(&warm_run));
    assert!(
        warm_pixel[0] / warm_pixel[2] > 1.03 * neutral_pixel[0] / neutral_pixel[2],
        "«+ теплота» делает серый теплее: нейтраль {neutral_pixel:?}, тёплый {warm_pixel:?}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn neutral_look_is_bit_identical_to_the_base_and_switching_does_not_change_the_pipeline_variant() {
    let Some(context) = gpu_context_or_skip() else { return };
    for (base_name, base) in bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let raw = hue_sweep(&spec);
        let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
        let neutral = spec.build();
        let touched = spec.build_with_look(saturation_vibrance(0.7, 0.5));
        assert!(neutral.look.color.is_none());
        assert!(touched.look.color.is_some());

        // Нейтральный вид = картинка основы: набор без стадий вида даёт ту же картинку бит в бит.
        let product = run_viewfinder(
            &context,
            &config,
            &RunRequest::whole_frame(&raw, &neutral, spec.display()),
        );
        let without_look = run_viewfinder(
            &context,
            &config,
            &RunRequest {
                stages: IspStageSet::ALL.without(IspStage::LookColor).without(IspStage::LookLut),
                ..RunRequest::whole_frame(&raw, &neutral, spec.display())
            },
        );
        assert_eq!(product.pixels, without_look.pixels, "{base_name}: нейтраль ≠ основа");

        // Один ISP: нейтраль → вид → нейтраль. Вариант pipeline один, картинка нейтрали возвращается бит в бит.
        let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
        let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
        let source = gpu_source.source();
        // нейтраль → цвет → цвет+колёса → цвет → только колёса → нейтраль: один pipeline, ни одна стадия не
        // «залипает» в uniform между кадрами.
        let colour_and_wheels = spec.build_with_look(controls(LookSettings {
            wheels: all_wheels(),
            saturation: knob(0.7),
            vibrance: knob(0.5),
            ..LookSettings::NEUTRAL
        }));
        let wheels_only = spec.build_with_look(wheels_look(all_wheels()));
        assert!(wheels_only.look.color.is_some(), "колёса сами включают стадию цвета");
        let sequence = [&neutral, &touched, &colour_and_wheels, &touched, &wheels_only, &neutral];
        let mut runs = Vec::new();
        for (frame, params) in sequence.into_iter().enumerate() {
            viewfinder_support::record_and_submit(
                &context,
                &mut isp,
                &RunRequest::whole_frame(&raw, params, spec.display()),
                &source,
                frame % 2,
                gpu_vulkan::backdrop::BackdropFrameNumber(frame as u64 + 1),
            );
            runs.push(viewfinder_support::readback(&mut isp, frame % 2));
        }
        assert_ne!(runs[0].pixels, runs[1].pixels, "{base_name}: вид не изменил картинку");
        assert_ne!(
            runs[1].pixels, runs[2].pixels,
            "{base_name}: колёса не изменили картинку"
        );
        assert_ne!(
            runs[0].pixels, runs[4].pixels,
            "{base_name}: колёса без цвета не изменили картинку"
        );
        assert_eq!(
            runs[1].pixels, runs[3].pixels,
            "{base_name}: после колёс вид без колёс не вернулся бит-в-бит (строки колёс залипли)"
        );
        assert_eq!(
            runs[0].pixels, runs[5].pixels,
            "{base_name}: возврат к нейтрали не бит-в-бит"
        );
        assert_eq!(
            runs[0].pixels, product.pixels,
            "{base_name}: разные ISP дали разную нейтраль"
        );
        assert_eq!(
            isp.cached_pipeline_variants(),
            1,
            "{base_name}: смена «нейтраль ↔ вид ↔ колёса» создала новый вариант pipeline"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn identity_table_keeps_the_picture_and_known_table_matches_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    // «Единица»: узел равен координате. Картинка отличается от нейтральной только округлением кодирования
    // (в пределах шага 10 бит) — и совпадает с эталоном, который делает то же.
    let identity = with_lut(LookControls::default(), identity_lut());
    let known = with_lut(LookControls::default(), known_lut());
    let known_with_colour = with_lut(saturation_vibrance(0.5, 0.3), known_lut());
    for (name, look) in [
        ("таблица-единица", &identity),
        ("известная таблица", &known),
        ("известная таблица + цвет", &known_with_colour),
    ] {
        if name != "таблица-единица" {
            // Известная таблица меняет картинку заметно (единица — только округлением кодирования).
            assert_look_changes_the_picture(name, look);
        }
        let worst = assert_look_parity(&context, name, look);
        eprintln!("{name}: худший max |Δ| fp16 = {worst:.3e}");
    }
    // Единица почти не меняет картинку: на серой шкале (свет W внутри 0…1) разница против нейтрали — доли LSB
    // (только округление кодирования). Цвета вне 0…1 таблица обрезает на входе (документировано в `look/lut.rs`).
    for (base_name, base) in bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let raw = &synthetic_scenes(&spec)[0].1;
        let plain = reference_upright_encoded(raw, raw.full_rect(), SensorOrientation::Degrees0, &spec.build());
        let through_identity = reference_upright_encoded(
            raw,
            raw.full_rect(),
            SensorOrientation::Degrees0,
            &spec.build_with_look(identity.clone()),
        );
        // Только серые пиксели не у белого и не у чёрного: у краёв AgX отдаёт W чуть вне 0…1 (например, синий ниже 0 у
        // самой тени), а таблица обрезает вход в 0…1 — это документированная граница домена, а не ошибка.
        let biggest = plain
            .pixels
            .iter()
            .zip(&through_identity.pixels)
            .filter(|(plain_pixel, _)| {
                let (lowest, highest) = plain_pixel
                    .iter()
                    .fold((1.0_f32, 0.0_f32), |(low, high), c| (low.min(*c), high.max(*c)));
                highest < 0.85 && highest - lowest < 0.02
            })
            .flat_map(|(left, right)| left.iter().zip(right).map(|(a, b)| (a - b).abs()))
            .fold(0.0_f32, f32::max);
        assert!(
            biggest < 1.0 / 1023.0,
            "{base_name}: таблица-единица сдвигает серую шкалу на {biggest}"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn look_stage_switches_cut_exactly_their_stage() {
    let Some(context) = gpu_context_or_skip() else { return };
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    for (base_name, base) in bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let raw = hue_sweep(&spec);
        let colour_only = spec.build_with_look(saturation_vibrance(0.9, 0.0));
        let lut_only = spec.build_with_look(with_lut(LookControls::default(), known_lut()));
        let both = spec.build_with_look(with_lut(saturation_vibrance(0.9, 0.0), known_lut()));
        let neutral = spec.build();
        let run = |params: &isp_model::params::IspParams, stages: IspStageSet| {
            run_viewfinder(
                &context,
                &config,
                &RunRequest {
                    stages,
                    ..RunRequest::whole_frame(&raw, params, spec.display())
                },
            )
        };
        let all = IspStageSet::ALL;
        // Цвет: вырезание стадии возвращает картинку без вида; у чистой таблицы стадия цвета ничего не режет.
        assert_eq!(
            run(&colour_only, all.without(IspStage::LookColor)).pixels,
            run(&neutral, all).pixels,
            "{base_name}: без стадии цвета картинка равна нейтральной"
        );
        assert_ne!(run(&colour_only, all).pixels, run(&neutral, all).pixels, "{base_name}");
        assert_eq!(
            run(&lut_only, all.without(IspStage::LookColor)).pixels,
            run(&lut_only, all).pixels,
            "{base_name}: у чистой таблицы стадия цвета ничего не делает"
        );
        // Таблица: вырезание стадии возвращает картинку без таблицы; у цвета без таблицы стадия ничего не режет.
        assert_eq!(
            run(&lut_only, all.without(IspStage::LookLut)).pixels,
            run(&neutral, all).pixels,
            "{base_name}: без стадии таблицы картинка равна нейтральной"
        );
        assert_ne!(run(&lut_only, all).pixels, run(&neutral, all).pixels, "{base_name}");
        assert_eq!(
            run(&colour_only, all.without(IspStage::LookLut)).pixels,
            run(&colour_only, all).pixels,
            "{base_name}: без таблицы в параметрах стадия таблицы пуста"
        );
        // Обе: каждая стадия по отдельности даёт свою часть вида (цвет без таблицы / таблица без цвета).
        let both_all = run(&both, all).pixels;
        assert_ne!(
            run(&both, all.without(IspStage::LookColor)).pixels,
            both_all,
            "{base_name}"
        );
        assert_ne!(
            run(&both, all.without(IspStage::LookLut)).pixels,
            both_all,
            "{base_name}"
        );
        assert_eq!(
            run(&both, all.without(IspStage::LookLut)).pixels,
            run(&colour_only, all).pixels,
            "{base_name}: цвет и таблица независимы"
        );
        assert_eq!(
            run(&both, all.without(IspStage::LookColor)).pixels,
            run(&lut_only, all).pixels,
            "{base_name}: таблица и цвет независимы"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn table_larger_than_the_buffer_is_an_explicit_error_and_the_isp_stays_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    config.look_lut = LookLutLimits { max_size: 5 };
    let spec = spec(ToneBase::Agx(AgxSettings::default()), DisplayGamut::DisplayP3);
    let raw = hue_sweep(&spec);
    let too_big = spec.build_with_look(with_lut(LookControls::default(), known_lut()));
    let fits = spec.build_with_look(with_lut(
        LookControls::default(),
        isp_model::look::lut::LookLut::identity(5, 65).unwrap(),
    ));
    let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
    let gpu_source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();
    let error = viewfinder_support::try_record(
        &context,
        &mut isp,
        &RunRequest::whole_frame(&raw, &too_big, spec.display()),
        &source,
        0,
    )
    .expect_err("таблица 9³ не влезает в буфер 5³");
    assert!(
        matches!(
            error,
            ViewfinderError::LookLut(isp_vulkan::look::LookLutError::TooLarge { size: 9, max: 5 })
        ),
        "{error:?}"
    );
    // ISP цел: следующий кадр с подходящей таблицей проходит и совпадает с эталоном.
    viewfinder_support::record_and_submit(
        &context,
        &mut isp,
        &RunRequest::whole_frame(&raw, &fits, spec.display()),
        &source,
        1,
        gpu_vulkan::backdrop::BackdropFrameNumber(2),
    );
    let run = viewfinder_support::readback(&mut isp, 1);
    let reference = reference_upright_encoded(&raw, raw.full_rect(), SensorOrientation::Degrees0, &fits);
    let result = viewfinder_support::parity_with_pixels(&run, reference.width, reference.height, &reference.pixels);
    assert!(
        result.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE,
        "{result:?}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn colour_wheels_match_reference_on_every_zone_and_edge() {
    let Some(context) = gpu_context_or_skip() else { return };
    for (name, look) in wheels_parity_cases() {
        assert_look_changes_the_picture(&name, &look);
        let worst = assert_look_parity(&context, &name, &look);
        eprintln!("{name}: худший max |Δ| fp16 = {worst:.3e}");
    }
    assert_no_validation_errors(&context);
}

/// Сколько зона вне колеса может сдвинуться «просто от прохода через OKLab» (перевод туда и обратно не бит-в-бит,
/// но это доли шага fp16, а не оттенок): на два порядка меньше самого малого видимого сдвига зоны (0,02).
const UNTOUCHED_ZONE_TOLERANCE: f32 = 2.0e-4;

/// Серая шкала: слева тёмное, справа светлое; колёса красят зоны, а не весь кадр.
#[test]
fn lift_changes_the_dark_part_only_and_gain_the_bright_part_on_the_gpu() {
    let Some(context) = gpu_context_or_skip() else { return };
    for (base_name, base) in bases() {
        let spec = spec(base, DisplayGamut::DisplayP3);
        let gray_ramp = &synthetic_scenes(&spec)[0].1;
        let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
        let run = |look: LookControls| {
            let params = spec.build_with_look(look);
            run_viewfinder(
                &context,
                &config,
                &RunRequest::whole_frame(gray_ramp, &params, spec.display()),
            )
        };
        let neutral = run(LookControls::default());
        // Выход закодирован (sRGB), и серый экрана (0,2 линейно) лежит уже на третьем столбце шкалы: тёмная часть —
        // два первых столбца, светлая — последняя четверть кадра (выше серого с запасом).
        let dark_columns = 0..2;
        let bright_columns = neutral.width * 3 / 4..neutral.width;
        let biggest_change = |other: &viewfinder_support::ViewfinderRun, columns: std::ops::Range<u32>| {
            let mut biggest = 0.0_f32;
            for y in 0..neutral.height {
                for x in columns.clone() {
                    let (left, right) = (neutral.pixel(x, y), other.pixel(x, y));
                    for channel in 0..3 {
                        biggest = biggest.max((left[channel] - right[channel]).abs());
                    }
                }
            }
            biggest
        };
        let lifted = run(single_wheel_look(WheelZone::Lift, wheel(0.0, 0.0, 1.0)));
        assert!(
            biggest_change(&lifted, dark_columns.clone()) > 0.02,
            "{base_name}: Lift не осветлил тени"
        );
        assert!(
            biggest_change(&lifted, bright_columns.clone()) < UNTOUCHED_ZONE_TOLERANCE,
            "{base_name}: Lift тронул света (вес теней выше серого — ровно 0)"
        );
        // Кольцо −1: светлее белого у прежней основы кадр обрезается, и осветление света было бы не видно.
        let gained = run(single_wheel_look(WheelZone::Gain, wheel(0.0, 0.0, -1.0)));
        assert!(
            biggest_change(&gained, bright_columns) > 0.02,
            "{base_name}: Gain не затемнил света"
        );
        assert!(
            biggest_change(&gained, dark_columns) < UNTOUCHED_ZONE_TOLERANCE,
            "{base_name}: Gain тронул тени (вес светов ниже серого — ровно 0)"
        );
    }
    assert_no_validation_errors(&context);
}
