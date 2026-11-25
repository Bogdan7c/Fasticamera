//! Сквозной паритет качественного конвейера (V4) с эталоном `isp-model` для основы ACES и своего белого
//! точки (решения 176, 177, 182): RAW → уровни → баланс с обрезкой по `L(pos)` → Menon 2007 → XYZ → W → ACES
//! (свечение, красный модификатор, сплайны RRT и ODT, окружение) → вид → цвета выхода → OETF. Допуски — как у AgX и прежней основы
//! (`quality_pipeline_parity.rs`): линейный RGB камеры (в нём сидит обрезка по `L`) — бит в бит, сигнал экрана —
//! 1e-3 (шаг fp16 у 1,0 = 4,9e-4). Обе цели вывода, настройки модели и ручки тона на краях, слой вида поверх.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod lens_shading_support;
mod look_support;
mod quality_support;

use camera_contract::geometry::PixelSize;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::look::LookControls;
use isp_model::params::{ExposureGain, FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{WorkingReferenceRun, run_reference_working};
use isp_model::sensor::SensorDescription;
use isp_model::test_patterns::zone_plate;
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::base::ToneBase;
use isp_support::real_capture::load_real_capture;
use lens_shading_support::{asymmetric_map, with_map};
use look_contract::LookSettings;
use quality_support::{
    area_of, assert_no_validation_errors, gpu_context_or_skip, max_rgb_difference, padded_frame, run_gpu,
};

/// Линейный RGB камеры: бит в бит (в нём — обрезка баланса по белому точки).
const LINEAR_TOLERANCE: f32 = 0.0;
/// Сигнал экрана (fp16-цель): четверть шага 8 бит, как у прежней основы.
const DISPLAY_TOLERANCE: f32 = 1e-3;
/// Наименьший размах сигнала эталона (экранный код 0…1): картинка не плоская, паритет не пуст. Не допуск паритета.
const MIN_REFERENCE_RANGE: f32 = 0.2;
/// То же для углов ручек тона: стадия тона на краях честно сжимает кадр (все ручки в минус: реальный кроп с пересветом
/// 0,0008…0,097; тени в плюс при остальных в минус: 0,058…0,097). От вырожденной картинки паритет углов защищён
/// проверкой «разные углы — разные картинки», а здесь нужен лишь запрет константы.
const TONED_MIN_REFERENCE_RANGE: f32 = 0.02;

fn aces_params_with(
    sensor: &SensorDescription,
    neutral: CameraNeutral,
    gamut: DisplayGamut,
    settings: AcesSettings,
    look: LookControls,
    exposure_ev: f32,
) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
    };
    let controls = ProcessingControls {
        tone_base: ToneBase::Aces(settings),
        exposure_gain: ExposureGain::from_ev(exposure_ev).unwrap(),
        look,
        ..ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, gamut)
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

fn aces_params(sensor: &SensorDescription, neutral: CameraNeutral, gamut: DisplayGamut) -> IspParams {
    aces_params_with(
        sensor,
        neutral,
        gamut,
        AcesSettings::default(),
        LookControls::default(),
        0.0,
    )
}

fn assert_aces_parity(name: &str, run: &WorkingReferenceRun, linear: &[[f32; 3]], display: &[[f32; 3]]) {
    assert_aces_parity_in_range(name, run, linear, display, MIN_REFERENCE_RANGE);
}

/// Паритет с наименьшим размахом эталона `min_range` (см. [`TONED_MIN_REFERENCE_RANGE`]).
fn assert_aces_parity_in_range(
    name: &str,
    run: &WorkingReferenceRun,
    linear: &[[f32; 3]],
    display: &[[f32; 3]],
    min_range: f32,
) {
    let width = run.front_end.camera_rgb.size().width;
    let (linear_difference, linear_at) = max_rgb_difference(linear, run.front_end.camera_rgb.pixels(), width);
    let (display_difference, display_at) = max_rgb_difference(display, run.display_encoded.pixels(), width);
    println!(
        "{name}: линейный RGB max |Δ| {linear_difference:e} в {linear_at:?}; сигнал экрана max |Δ| \
         {display_difference:e} в {display_at:?}"
    );
    assert!(
        linear_difference <= LINEAR_TOLERANCE,
        "{name}: линейный {linear_difference:e} в {linear_at:?}"
    );
    assert!(
        display_difference <= DISPLAY_TOLERANCE,
        "{name}: экран {display_difference:e} в {display_at:?}"
    );
    let encoded = run.display_encoded.pixels();
    let brightest = encoded.iter().flatten().copied().fold(0.0_f32, f32::max);
    let darkest = encoded.iter().flatten().copied().fold(1.0_f32, f32::min);
    assert!(
        brightest - darkest > min_range,
        "{name}: эталон слишком плоский ({darkest}..{brightest})"
    );
}

/// Эталон доходит до света выше общего белого: иначе тест не проверял бы свой белый точки.
fn assert_reaches_per_point_white(name: &str, run: &WorkingReferenceRun) {
    let brightest = run
        .front_end
        .camera_rgb
        .pixels()
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f32, f32::max);
    assert!(
        brightest > 1.2,
        "{name}: кадр не доходит до белого точки выше 1 (max {brightest})"
    );
}

/// Прогоняет мозаику через GPU из обоих источников (RAW16 и упакованный RAW10) и сверяет с эталоном; возвращает
/// сигнал экрана RAW16 (для сравнения картинок между собой).
fn run_both_sources(
    context: &gpu_vulkan::context::GpuContext,
    name: &str,
    raw: &RawMosaic,
    params: &IspParams,
    reference: &WorkingReferenceRun,
) -> Vec<[f32; 3]> {
    run_both_sources_in_range(context, name, raw, params, reference, MIN_REFERENCE_RANGE)
}

/// То же с наименьшим размахом эталона `min_range`.
fn run_both_sources_in_range(
    context: &gpu_vulkan::context::GpuContext,
    name: &str,
    raw: &RawMosaic,
    params: &IspParams,
    reference: &WorkingReferenceRun,
    min_range: f32,
) -> Vec<[f32; 3]> {
    let (frame, frame_width, frame_height) = padded_frame(raw, 1023);
    let raw16 = SyntheticRaw16::new(context, frame_width, frame_height, &frame).unwrap();
    let (linear, display) = run_gpu(context, &raw16.source(), params, area_of(raw));
    assert_aces_parity_in_range(&format!("{name}/raw16"), reference, &linear, &display, min_range);
    let row_stride_bytes = frame_width / 4 * 5 + 6;
    let packed = pack_raw10(&frame, frame_width, row_stride_bytes, 0xA5);
    let raw10 = SyntheticRaw10::new(context, frame_width, frame_height, row_stride_bytes, &packed).unwrap();
    let source: MosaicSource = raw10.source();
    let (linear10, display10) = run_gpu(context, &source, params, area_of(raw));
    assert_aces_parity_in_range(&format!("{name}/raw10"), reference, &linear10, &display10, min_range);
    display
}

const REAL_CROPS: [&str; 3] = [
    "real-main-lamp-jar",
    "real-main-lamp-towel",
    "real-main-lamp-tap-highlight",
];

#[test]
fn real_crops_without_correction_match_reference_for_both_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        for name in REAL_CROPS {
            let capture = load_real_capture(&isp_support::assets_dir(), name);
            let params = aces_params(&capture.sensor, capture.neutral, gamut);
            let reference = run_reference_working(&capture.raw, &params).unwrap();
            run_both_sources(
                &context,
                &format!("{name}/{gamut:?}"),
                &capture.raw,
                &params,
                &reference,
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn shaded_real_crops_use_own_white_and_match_reference_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    {
        for name in REAL_CROPS {
            let capture = load_real_capture(&isp_support::assets_dir(), name);
            let plain = aces_params(&capture.sensor, capture.neutral, DisplayGamut::Srgb);
            let params = with_map(&plain, &asymmetric_map(5, 4));
            let reference = run_reference_working(&capture.raw, &params).unwrap();
            // Карта меняет эталон: иначе совпадение ничего бы не доказывало.
            let unshaded = run_reference_working(&capture.raw, &plain).unwrap();
            let (effect, _) = max_rgb_difference(
                reference.front_end.camera_rgb.pixels(),
                unshaded.front_end.camera_rgb.pixels(),
                48,
            );
            assert!(effect > 0.02, "{name}: карта почти не меняет эталон ({effect})");
            run_both_sources(&context, &format!("{name}/shaded"), &capture.raw, &params, &reference);
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn saturated_zone_plate_with_map_matches_reference_for_every_cfa_layout() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let scene = zone_plate(PixelSize::new(64, 48), 0.5).unwrap();
    // Карта 7×6: узлы не совпадают с пикселями, усиления до ~2,3 — яркие места насыщают сенсор после коррекции.
    let map = asymmetric_map(7, 6);
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let sensor = SensorDescription::new(
            pattern,
            calibration.sensor.static_levels(),
            *calibration.sensor.color_profile(),
            calibration.sensor.vendor_lens_shading(),
        );
        let levels = sensor.static_levels();
        let neutral = calibration.neutral.values();
        // Свет доходит до насыщения сенсора (множитель 1,3 · нейтраль): часть зонной пластинки — в пересвете.
        let samples: Vec<u16> = (0..scene.size().height)
            .flat_map(|y| (0..scene.size().width).map(move |x| (x, y)))
            .map(|(x, y)| {
                let channel = pattern.channel_at(x, y).index();
                let light = (f64::from(scene.pixel(x, y)[channel]) * 1.3 * neutral[channel]).min(1.0);
                let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
                (black + light * (f64::from(levels.white()) - black)).round() as u16
            })
            .collect();
        let raw = RawMosaic::new(scene.size(), pattern, samples).unwrap();
        {
            let params = with_map(&aces_params(&sensor, calibration.neutral, DisplayGamut::Srgb), &map);
            let reference = run_reference_working(&raw, &params).unwrap();
            assert_reaches_per_point_white(&format!("zone_plate/{pattern:?}"), &reference);
            let (frame, frame_width, frame_height) = padded_frame(&raw, 0);
            let raw16 = SyntheticRaw16::new(&context, frame_width, frame_height, &frame).unwrap();
            let (linear, display) = run_gpu(&context, &raw16.source(), &params, area_of(&raw));
            assert_aces_parity(&format!("zone_plate/shaded/{pattern:?}"), &reference, &linear, &display);
        }
    }
    assert_no_validation_errors(&context);
}

/// Развёртка по оттенку (насыщенные цвета всех оттенков, от тёмных к светлым): свечение и красный модификатор
/// работают; обе цели вывода совпадают с эталоном и дают разные картинки.
#[test]
fn hue_sweep_matches_reference_for_both_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let mut pictures = Vec::new();
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let params = aces_params(&calibration.sensor, calibration.neutral, gamut);
        let reference = run_reference_working(&sweep, &params).unwrap();
        let display = run_both_sources(&context, &format!("развёртка/{gamut:?}"), &sweep, &params, &reference);
        pictures.push(display);
    }
    assert_ne!(pictures[0], pictures[1], "sRGB и Display P3 дали одну картинку");
    assert_no_validation_errors(&context);
}

/// Ручки тона на краях: углы {−1, +1}⁴ (контраст, средние, тени, света) × подтяжка AE ±0,5 EV на развёртке по
/// оттенку, серой шкале и реальном кропе с пересветом; кривая на разных углах разная.
#[test]
fn tone_knob_corners_with_ae_edges_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let ramp = look_support::gray_ramp_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let highlight = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-tap-highlight");
    let mut ramp_pictures = Vec::new();
    for exposure_ev in [-0.5_f32, 0.5] {
        for corner in 0..16_u32 {
            let value = |bit: u32| look_support::knob(if corner >> bit & 1 == 1 { 1.0 } else { -1.0 });
            let look = look_support::controls(LookSettings {
                contrast: value(0),
                midtones: value(1),
                shadows: value(2),
                highlights: value(3),
                ..LookSettings::NEUTRAL
            });
            let name = format!("ручки угол {corner:04b} AE {exposure_ev:+}");
            let params = aces_params_with(
                &calibration.sensor,
                calibration.neutral,
                DisplayGamut::Srgb,
                AcesSettings::default(),
                look.clone(),
                exposure_ev,
            );
            let reference = run_reference_working(&sweep, &params).unwrap();
            run_both_sources_in_range(
                &context,
                &format!("{name}/развёртка"),
                &sweep,
                &params,
                &reference,
                TONED_MIN_REFERENCE_RANGE,
            );
            let ramp_reference = run_reference_working(&ramp, &params).unwrap();
            let ramp_picture = run_both_sources_in_range(
                &context,
                &format!("{name}/шкала"),
                &ramp,
                &params,
                &ramp_reference,
                TONED_MIN_REFERENCE_RANGE,
            );
            if exposure_ev < 0.0 {
                ramp_pictures.push(ramp_picture);
            }
            let params = aces_params_with(
                &highlight.sensor,
                highlight.neutral,
                DisplayGamut::DisplayP3,
                AcesSettings::default(),
                look,
                exposure_ev,
            );
            let reference = run_reference_working(&highlight.raw, &params).unwrap();
            run_both_sources_in_range(
                &context,
                &format!("{name}/пересвет"),
                &highlight.raw,
                &params,
                &reference,
                TONED_MIN_REFERENCE_RANGE,
            );
        }
    }
    for (index, picture) in ramp_pictures.iter().enumerate() {
        for other in &ramp_pictures[index + 1..] {
            assert_ne!(picture, other, "два угла ручек дали одну картинку");
        }
    }
    assert_no_validation_errors(&context);
}

/// Слой вида поверх ACES: цвет (насыщенность, сочность), полосы и таблица — паритет с эталоном; вид меняет картинку.
#[test]
fn look_layer_on_top_of_aces_matches_reference_and_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let neutral_params = aces_params(&calibration.sensor, calibration.neutral, DisplayGamut::Srgb);
    let neutral_reference = run_reference_working(&sweep, &neutral_params).unwrap();
    let neutral_picture = run_both_sources(&context, "вид нейтрален", &sweep, &neutral_params, &neutral_reference);
    let looks = [
        ("насыщенность и сочность", look_support::saturation_vibrance(0.6, 0.3)),
        ("полосы", look_support::all_bands(0.3, 0.4, -0.2)),
        (
            "таблица",
            look_support::with_lut(look_support::saturation_vibrance(0.3, 0.0), look_support::known_lut()),
        ),
    ];
    for (name, look) in looks {
        let params = aces_params_with(
            &calibration.sensor,
            calibration.neutral,
            DisplayGamut::Srgb,
            AcesSettings::default(),
            look,
            0.0,
        );
        let reference = run_reference_working(&sweep, &params).unwrap();
        let picture = run_both_sources(&context, &format!("вид {name}"), &sweep, &params, &reference);
        assert_ne!(picture, neutral_picture, "вид {name} не меняет картинку ACES");
    }
    assert_no_validation_errors(&context);
}

/// Настройки модели ACES на краях допустимых диапазонов (по одной за раз): паритет держится на развёртке по оттенку
/// и тёмной насыщенной мозаике (там работает свечение), и каждая настройка меняет картинку.
#[test]
fn model_settings_at_range_edges_match_reference_and_each_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let sweep = look_support::hue_sweep_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let ramp = look_support::gray_ramp_mosaic(&calibration.sensor, calibration.neutral, 64, 48);
    let default = AcesSettings::default();
    let edges = [
        (
            "контраст",
            AcesSettings {
                contrast: 1.5,
                ..default
            },
        ),
        (
            "тени",
            AcesSettings {
                shadows_ev: 3.0,
                ..default
            },
        ),
        (
            "света",
            AcesSettings {
                highlights_ev: -2.0,
                ..default
            },
        ),
        (
            "серый экрана",
            AcesSettings {
                display_middle_gray: 0.3,
                ..default
            },
        ),
        (
            "колено",
            AcesSettings {
                knee_ev: 2.5,
                ..default
            },
        ),
        (
            "без свечения",
            AcesSettings {
                glow_gain: 0.0,
                ..default
            },
        ),
        (
            "красный модификатор",
            AcesSettings {
                red_modifier_scale: 0.0,
                ..default
            },
        ),
        (
            "десатурация RRT",
            AcesSettings {
                rrt_saturation: 0.5,
                ..default
            },
        ),
        (
            "десатурация выхода",
            AcesSettings {
                odt_saturation: 1.0,
                ..default
            },
        ),
        (
            "окружение",
            AcesSettings {
                surround_gamma: 0.75,
                ..default
            },
        ),
    ];
    let picture_with = |name: &str, settings: AcesSettings| -> Vec<[f32; 3]> {
        let mut pictures = Vec::new();
        for (scene, raw) in [("развёртка", &sweep), ("шкала", &ramp)] {
            let params = aces_params_with(
                &calibration.sensor,
                calibration.neutral,
                DisplayGamut::Srgb,
                settings,
                LookControls::default(),
                0.0,
            );
            let reference = run_reference_working(raw, &params).unwrap();
            pictures.extend(run_both_sources(
                &context,
                &format!("{name}/{scene}"),
                raw,
                &params,
                &reference,
            ));
        }
        pictures
    };
    let default_picture = picture_with("умолчание", default);
    for (name, settings) in edges {
        let picture = picture_with(name, settings);
        assert_ne!(picture, default_picture, "{name}: настройка не дошла до картинки");
    }
    assert_no_validation_errors(&context);
}
