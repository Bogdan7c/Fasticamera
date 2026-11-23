//! Паритет ISP видоискателя на GPU (RADV, validation) с эталоном `isp-model` (Preview) для основы ACES и своего
//! белого точки (решения 176, 177, 182): те же допуски, что у AgX и прежней основы: max |Δ| ≤ ½ LSB 10-битного
//! выхода + 1e-5, ноль пикселей хуже 1 LSB.
//!
//! Что покрыто: синтетика (серая шкала, полосы, пересветы, около чёрного, развёртка по оттенку — там работают
//! свечение и красный модификатор — и яркие насыщенные полосы), реальные кропы, карта виньетирования с яркими
//! пятнами в углу (свой белый точки выше 1), обе цели вывода, настройки модели и ручки тона на краях диапазонов
//! с подтяжкой AE на краях, слой вида поверх ACES, вырезание стадий, кадрирование и повороты, переключение
//! «старая основа → ACES → AgX» на одном ISP.

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
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::look::LookControls;
use isp_model::params::{ExposureGain, FrameInputs, IspParams, ProcessingControls};
use isp_model::tone::aces::AcesSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::working_io::WorkingInputGamut;
use isp_vulkan::viewfinder::ViewfinderStorageFormat;
use lens_shading_support::{asymmetric_map, with_map};
use look_contract::LookSettings;
use viewfinder_support::{
    HEIGHT_OF_SYNTHETIC_SCENES as HEIGHT, ParamsSpec, RawFrame, RunRequest, WIDTH_OF_SYNTHETIC_SCENES as WIDTH,
    assert_no_validation_errors, assert_viewfinder_parity, gpu_context_or_skip, isp_support, parity_with_pixels,
    reference_upright_encoded, run_viewfinder, synthetic_scenes, test_config,
};

fn aces_spec_with(gamut: DisplayGamut, settings: AcesSettings) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: ToneBase::Aces(settings),
        ..ParamsSpec::default()
    }
}

fn aces_spec(gamut: DisplayGamut) -> ParamsSpec {
    aces_spec_with(gamut, AcesSettings::default())
}

/// ACES с мягким сжатием охвата на входе (у настоящей камеры цвета внутри Rec. 2020 и обрезка нулём ничего не
/// меняет, а сжатие меняет цвета у границы охвата).
fn aces_soft_input_spec(gamut: DisplayGamut) -> ParamsSpec {
    aces_spec_with(
        gamut,
        AcesSettings {
            input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
            ..AcesSettings::default()
        },
    )
}

/// Развёртка по оттенку (насыщенные цвета всех оттенков, от тёмных к светлым): свечение и красный модификатор работают.
fn hue_sweep_frame(spec: &ParamsSpec) -> RawFrame {
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        look_support::hue_sweep_light(x, y, WIDTH, HEIGHT, channel, spec.neutral)
    })
}

/// Яркие насыщенные цвета: полосы с яркостью от почти чёрного до сильного пересвета. Заходят глубоко в плечо
/// кривой и за охват выхода (десатурации RRT и выхода, выходное сжатие охвата).
fn bright_saturated_frame(spec: &ParamsSpec) -> RawFrame {
    let bars =
        isp_model::test_patterns::color_bars(camera_contract::geometry::PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.02 + 1.5 * f64::from(y) / f64::from(HEIGHT - 1))
    })
}

/// Тёмные насыщенные цвета (полосы с яркостью от 1 до 8 % белого): здесь работает свечение RRT (оно действует
/// только на тёмное и насыщенное).
fn dark_saturated_frame(spec: &ParamsSpec) -> RawFrame {
    let bars =
        isp_model::test_patterns::color_bars(camera_contract::geometry::PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.01 + 0.07 * f64::from(y) / f64::from(HEIGHT - 1))
    })
}

/// Сцены паритета: четыре синтетические, развёртка по оттенку, яркие насыщенные полосы.
fn all_scenes(spec: &ParamsSpec) -> Vec<(&'static str, RawFrame)> {
    let mut scenes = synthetic_scenes(spec);
    scenes.push(("развёртка по оттенку", hue_sweep_frame(spec)));
    scenes.push(("яркие насыщенные", bright_saturated_frame(spec)));
    scenes.push(("тёмные насыщенные", dark_saturated_frame(spec)));
    scenes
}

#[test]
fn synthetic_scenes_match_reference_for_both_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
        let spec = aces_spec(gamut);
        let params = spec.build();
        assert!(matches!(
            params.tone_base,
            isp_model::tone::base::ResolvedToneBase::Aces(_)
        ));
        for (name, raw) in all_scenes(&spec) {
            let (half, ten_bit) =
                assert_viewfinder_parity(&context, &format!("ACES {gamut:?} {name}"), &raw, &params, &spec);
            eprintln!("ACES {gamut:?} {name}: max |Δ| fp16 {half:.3e}, 10 бит {ten_bit:.3e}");
        }
    }
    // Мягкое сжатие охвата на входе: паритет (насыщенные полосы и развёртка выходят к границе охвата).
    let soft = aces_soft_input_spec(DisplayGamut::Srgb);
    let soft_params = soft.build();
    for (name, raw) in all_scenes(&soft) {
        assert_viewfinder_parity(&context, &format!("ACES вход-сжатие {name}"), &raw, &soft_params, &soft);
    }
    assert_no_validation_errors(&context);
}

#[test]
fn real_crops_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    for crop in ["gray-tile", "towel", "jar", "tap-highlight"] {
        let capture =
            isp_support::real_capture::load_real_capture(&isp_support::assets_dir(), &format!("real-main-lamp-{crop}"));
        let frame = FrameInputs {
            levels: capture.sensor.static_levels(),
        };
        let raw = RawFrame {
            width: capture.raw.size().width,
            height: capture.raw.size().height,
            pattern: capture.raw.pattern(),
            samples: capture.raw.samples().to_vec(),
        };
        let controls = ProcessingControls {
            tone_base: ToneBase::Aces(AcesSettings::default()),
            ..ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3)
        };
        let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
        assert_viewfinder_parity(&context, crop, &raw, &params, &aces_spec(DisplayGamut::DisplayP3));
    }
    assert_no_validation_errors(&context);
}

/// Кадр: тёмный серый фон и яркие насыщенные пятна — в правом нижнем углу (где карта усиливает сильнее всего)
/// и в центре. Насыщенный сенсор после коррекции и баланса уходит выше 1: без своего белого точки угол
/// обрезался бы общим 1,0.
fn corner_spot_frame(spec: &ParamsSpec) -> RawFrame {
    let neutral = spec.neutral;
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        let in_corner = x >= WIDTH - 14 && y >= HEIGHT - 12;
        let in_centre = (WIDTH / 2 - 5..WIDTH / 2 + 5).contains(&x) && (HEIGHT / 2 - 5..HEIGHT / 2 + 5).contains(&y);
        if in_corner || in_centre {
            1.0
        } else {
            0.08 * neutral[channel] / brightest
        }
    })
}

#[test]
fn shading_map_with_corner_spots_uses_own_white_and_matches_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
        {
            let spec = aces_spec(gamut);
            let params = with_map(&spec.build(), &asymmetric_map(5, 4));
            let raw = corner_spot_frame(&spec);
            assert_viewfinder_parity(&context, &format!("карта и пятна {gamut:?}"), &raw, &params, &spec);

            // Пятно в углу — насыщенный сенсор: свой белый доводит его ровно до белого экрана (а не до серого тона
            // «выше 1, обрезано»). Проверяем по GPU, а не только по эталону.
            let run = run_viewfinder(
                &context,
                &test_config(ViewfinderStorageFormat::HalfFloatOnly),
                &RunRequest::whole_frame(&raw, &params, spec.display()),
            );
            let corner = run.pixel(run.width - 3, run.height - 3);
            for (channel, value) in corner.iter().take(3).enumerate() {
                assert!(
                    *value >= 0.99,
                    "{gamut:?}: угловое пятно должно быть белым, канал {channel} = {value}"
                );
            }
            let background = run.pixel(3, 3);
            assert!(
                background[1] < 0.6,
                "{gamut:?}: серый фон не должен быть белым: {background:?}"
            );
        }
    }
    assert_no_validation_errors(&context);
}

#[test]
fn one_isp_switches_between_bases_and_each_matches_its_own_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let legacy_spec = ParamsSpec::default();
    let aces = aces_spec(DisplayGamut::DisplayP3);
    let agx = ParamsSpec {
        tone_base: ToneBase::Agx(AgxSettings::default()),
        ..ParamsSpec::default()
    };
    let map = asymmetric_map(5, 4);
    let legacy_params = with_map(&legacy_spec.build(), &map);
    let aces_params = with_map(&aces.build(), &map);
    let agx_params = with_map(&agx.build(), &map);
    let raw = corner_spot_frame(&aces);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);

    let mut isp = isp_vulkan::viewfinder::ViewfinderIsp::new(&context, &config).unwrap();
    let source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = source.source();
    let order = [&legacy_params, &aces_params, &agx_params, &aces_params, &legacy_params];
    let mut runs = Vec::new();
    for (frame, params) in order.into_iter().enumerate() {
        let request = RunRequest::whole_frame(&raw, params, aces.display());
        viewfinder_support::record_and_submit(
            &context,
            &mut isp,
            &request,
            &source,
            frame % 2,
            gpu_vulkan::backdrop::BackdropFrameNumber(frame as u64 + 1),
        );
        runs.push(viewfinder_support::readback(&mut isp, frame % 2));
    }
    // Картинки трёх основ различаются (иначе переключение ничего не доказывает).
    assert_ne!(runs[0].pixels, runs[1].pixels, "legacy и ACES дали одну картинку");
    assert_ne!(runs[1].pixels, runs[2].pixels, "ACES и AgX дали одну картинку");
    // Статистика AE/AWB меряет честную базу до основы тона (решение 175): от основы не зависит.
    assert!(runs[0].report.awb_statistics.is_some());
    assert_eq!(
        runs[0].report.awb_statistics, runs[1].report.awb_statistics,
        "статистика зависит от основы"
    );
    assert_eq!(
        runs[1].pixels, runs[3].pixels,
        "возврат на ACES — та же картинка бит в бит"
    );
    assert_eq!(
        runs[0].pixels, runs[4].pixels,
        "возврат на прежнюю основу — та же картинка бит в бит"
    );
    for (run, params) in runs.iter().zip(order) {
        let reference = reference_upright_encoded(&raw, raw.full_rect(), SensorOrientation::Degrees0, params);
        let result = parity_with_pixels(run, reference.width, reference.height, &reference.pixels);
        assert!(
            result.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE,
            "{result:?}"
        );
        assert_eq!(result.over_one_lsb, 0.0, "{result:?}");
    }
    assert_eq!(
        isp.cached_pipeline_variants(),
        3,
        "по варианту на основу (прежняя, ACES, AgX), без пересоздания"
    );
    assert_no_validation_errors(&context);
}

/// У основы ACES каждая стадия-выключатель вырезается и меняет картинку (кроме статистики AWB, которая картинку
/// не трогает): набор стадий действует и на этом варианте шейдера.
#[test]
fn every_stage_off_changes_the_aces_picture() {
    use isp_vulkan::viewfinder::{IspStage, IspStageSet};
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = aces_soft_input_spec(DisplayGamut::Srgb);
    // Вид с цветом и таблицей: иначе стадии вида нечего резать (при нейтральном виде они пусты).
    let look = look_support::with_lut(look_support::saturation_vibrance(0.6, 0.3), look_support::known_lut());
    let params = with_map(&spec.build_with_look(look), &asymmetric_map(5, 4));
    let raw = bright_saturated_frame(&spec);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let product = run_viewfinder(
        &context,
        &config,
        &RunRequest::whole_frame(&raw, &params, spec.display()),
    );
    for stage in IspStage::ALL {
        let request = RunRequest {
            stages: IspStageSet::ALL.without(stage),
            ..RunRequest::whole_frame(&raw, &params, spec.display())
        };
        let run = run_viewfinder(&context, &config, &request);
        if stage == IspStage::AwbStatistics {
            assert_eq!(run.pixels, product.pixels, "статистика не влияет на картинку");
        } else {
            assert_ne!(
                run.pixels, product.pixels,
                "без {stage:?} картинка ACES обязана измениться"
            );
        }
    }
    assert_no_validation_errors(&context);
}

/// Буфер кадра больше полезной области: отступы слева 4, сверху 2, справа 4, снизу 6.
const VALID_LEFT: u32 = 4;
const VALID_TOP: u32 = 2;
const BUFFER_WIDTH: u32 = VALID_LEFT + WIDTH + 4;
const BUFFER_HEIGHT: u32 = VALID_TOP + HEIGHT + 6;

/// Кадр: полезная область со световой «шашкой» и градиентом, паддинг — насыщение; в правом нижнем углу полезной
/// области — яркое пятно (сенсор насыщен, карта усиливает угол сильнее всего: свой белый точки там выше 1).
fn padded_buffer_frame(spec: &ParamsSpec) -> (RawFrame, camera_contract::geometry::PixelRect) {
    use camera_contract::geometry::{PixelRect, PixelSize};
    let valid = PixelRect {
        left: VALID_LEFT,
        top: VALID_TOP,
        size: PixelSize::new(WIDTH, HEIGHT),
    };
    let neutral = spec.neutral;
    let brightest = neutral.iter().copied().fold(f64::MIN, f64::max);
    let frame = RawFrame::from_linear(
        BUFFER_WIDTH,
        BUFFER_HEIGHT,
        spec.pattern,
        spec.levels(),
        |x, y, channel| {
            let inside = x >= VALID_LEFT && y >= VALID_TOP && x < VALID_LEFT + WIDTH && y < VALID_TOP + HEIGHT;
            if !inside {
                return 1.0;
            }
            let (ux, uy) = (x - VALID_LEFT, y - VALID_TOP);
            if ux >= WIDTH - 14 && uy >= HEIGHT - 12 {
                return 1.0;
            }
            let gradient = 0.5 * f64::from(ux) / f64::from(WIDTH - 1) + 0.5 * f64::from(uy) / f64::from(HEIGHT - 1);
            let checker = f64::from((ux / 4 + uy / 4) % 2);
            (0.05 + 0.15 * gradient + 0.05 * checker) * neutral[channel] / brightest
        },
    );
    (frame, valid)
}

/// Кадрирование внутри полезной области со смещением и каждым поворотом: ACES берёт карту виньетирования и белый
/// точки по координатам ПОЛЕЗНОЙ области (а не кропа и не буфера), как и остальные основы.
#[test]
fn crop_inside_offset_valid_area_and_every_rotation_match_the_aces_reference() {
    use camera_contract::geometry::{PixelRect, PixelSize};
    use viewfinder_support::{SourceLayout, extent, parity_with_pixels, reference_upright_encoded_in_valid_area};
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = aces_spec(DisplayGamut::DisplayP3);
    let params = with_map(&spec.build(), &asymmetric_map(6, 5));
    let (raw, valid) = padded_buffer_frame(&spec);
    let crop = PixelRect {
        left: VALID_LEFT + 24,
        top: VALID_TOP + 14,
        size: PixelSize::new(40, 34),
    };
    // Тест различает ошибку: эталон «кроп считается началом полезной области» отличается от верного больше 1 LSB.
    let right = reference_upright_encoded_in_valid_area(&raw, valid, crop, SensorOrientation::Degrees0, &params);
    let wrong = reference_upright_encoded(&raw, crop, SensorOrientation::Degrees0, &params);
    let biggest = right
        .pixels
        .iter()
        .zip(&wrong.pixels)
        .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
        .fold(0.0_f32, f32::max);
    assert!(
        biggest > 4.0 / 1023.0,
        "кроп в полезной области почти не отличается от кропа у начала ({biggest}): тест не ловит ошибку смещения"
    );
    for layout in [SourceLayout::Raw16, SourceLayout::Raw10 { padding_bytes: 6 }] {
        for orientation in [
            SensorOrientation::Degrees0,
            SensorOrientation::Degrees90,
            SensorOrientation::Degrees180,
            SensorOrientation::Degrees270,
        ] {
            let request = RunRequest {
                layout,
                geometry: isp_vulkan::viewfinder::ViewfinderGeometry {
                    valid_area: valid,
                    crop,
                    sensor_orientation: orientation,
                    allocation: isp_vulkan::viewfinder::OutputAllocation::Exact,
                },
                ..RunRequest::whole_frame(&raw, &params, spec.display())
            };
            let run = run_viewfinder(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly), &request);
            let reference = reference_upright_encoded_in_valid_area(&raw, valid, crop, orientation, &params);
            let result = parity_with_pixels(&run, reference.width, reference.height, &reference.pixels);
            assert_eq!(extent(&run).width, reference.width);
            assert!(
                result.math_excess <= viewfinder_support::MATH_EXCESS_TOLERANCE
                    && result.max_abs <= viewfinder_support::HALF_LSB + viewfinder_support::MATH_EXCESS_TOLERANCE,
                "{layout:?}/{orientation:?}: {result:?}"
            );
            assert_eq!(result.over_one_lsb, 0.0, "{layout:?}/{orientation:?}: {result:?}");
        }
    }
    assert_no_validation_errors(&context);
}

/// Ручки тона на краях: углы {−1, +1}⁴ ручек (контраст, средние, тени, света) × подтяжка AE на краях ±0,5 EV.
/// Каждая ручка двигает свои числа кривой (наклон, серый экрана, степени носка и плеча): паритет держится на всех
/// углах, а кривая на разных углах — разная.
#[test]
fn tone_knob_corners_with_ae_edges_match_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = aces_spec(DisplayGamut::DisplayP3);
    let scenes = [
        ("серая шкала", synthetic_scenes(&spec).remove(0).1),
        ("яркие насыщенные", bright_saturated_frame(&spec)),
    ];
    let mut pictures = Vec::new();
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
            let params = spec.build_with_look_and_exposure(look, ExposureGain::from_ev(exposure_ev).unwrap());
            for (name, raw) in &scenes {
                assert_viewfinder_parity(
                    &context,
                    &format!("ручки угол {corner:04b} AE {exposure_ev:+} {name}"),
                    raw,
                    &params,
                    &spec,
                );
            }
            if exposure_ev < 0.0 {
                let run = run_viewfinder(
                    &context,
                    &test_config(ViewfinderStorageFormat::HalfFloatOnly),
                    &RunRequest::whole_frame(&scenes[0].1, &params, spec.display()),
                );
                pictures.push(run.pixels);
            }
        }
    }
    // Углы действительно дают разные картинки: ручки доходят до GPU.
    for (index, picture) in pictures.iter().enumerate() {
        for other in &pictures[index + 1..] {
            assert_ne!(picture, other, "два угла ручек дали одну картинку");
        }
    }
    assert_no_validation_errors(&context);
}

/// Слой вида поверх ACES: цвет (насыщенность, сочность), полосы и таблица — паритет с эталоном и вид меняет
/// картинку против нейтрального; нейтральный вид — та же картинка, что без вида, бит в бит.
#[test]
fn look_layer_on_top_of_aces_matches_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = aces_spec(DisplayGamut::DisplayP3);
    let raw = hue_sweep_frame(&spec);
    let neutral_params = spec.build();
    let neutral = run_viewfinder(
        &context,
        &test_config(ViewfinderStorageFormat::HalfFloatOnly),
        &RunRequest::whole_frame(&raw, &neutral_params, spec.display()),
    );
    let looks = [
        ("насыщенность и сочность", look_support::saturation_vibrance(0.6, 0.3)),
        ("полосы", look_support::all_bands(0.3, 0.4, -0.2)),
        (
            "таблица",
            look_support::with_lut(look_support::saturation_vibrance(0.3, 0.0), look_support::known_lut()),
        ),
    ];
    for (name, look) in looks {
        let params = spec.build_with_look(look);
        for (scene, frame) in all_scenes(&spec) {
            assert_viewfinder_parity(&context, &format!("вид {name} / {scene}"), &frame, &params, &spec);
        }
        let run = run_viewfinder(
            &context,
            &test_config(ViewfinderStorageFormat::HalfFloatOnly),
            &RunRequest::whole_frame(&raw, &params, spec.display()),
        );
        assert_ne!(run.pixels, neutral.pixels, "вид {name} не меняет картинку ACES");
    }
    let again = spec.build_with_look(LookControls::default());
    let run = run_viewfinder(
        &context,
        &test_config(ViewfinderStorageFormat::HalfFloatOnly),
        &RunRequest::whole_frame(&raw, &again, spec.display()),
    );
    assert_eq!(run.pixels, neutral.pixels, "нейтральный вид меняет картинку");
    assert_no_validation_errors(&context);
}

/// Правка одной настройки модели ACES (для краевых значений).
type SettingsEdit = fn(&mut AcesSettings);

/// Настройки модели ACES на краях допустимых диапазонов (по одной за раз): паритет держится, и каждая настройка
/// доходит до шейдера — картинка развёртки по оттенку отличается от картинки с умолчаниями.
#[test]
fn model_settings_at_range_edges_match_reference_and_each_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    let default = AcesSettings::default();
    // Каждая запись — имя и правка одного поля умолчаний.
    let edges: [(&str, SettingsEdit); 21] = [
        ("контраст ниже", |s| s.contrast = 0.7),
        ("контраст выше", |s| s.contrast = 1.5),
        ("тени темнее", |s| s.shadows_ev = -3.0),
        ("тени светлее", |s| s.shadows_ev = 3.0),
        ("света темнее", |s| s.highlights_ev = -2.0),
        ("света светлее", |s| s.highlights_ev = 3.0),
        ("серый экрана ниже", |s| s.display_middle_gray = 0.1),
        ("серый экрана выше", |s| s.display_middle_gray = 0.3),
        ("колено ниже", |s| s.knee_ev = 0.8),
        ("колено выше", |s| s.knee_ev = 2.5),
        ("ширина перехода", |s| {
            (s.shape_width_ev, s.shadows_ev, s.highlights_ev) = (1.0, 1.0, -1.0);
        }),
        ("без свечения", |s| s.glow_gain = 0.0),
        ("сильное свечение", |s| s.glow_gain = 0.3),
        ("без красного модификатора", |s| {
            s.red_modifier_scale = 1.0
        }),
        ("сильный красный модификатор", |s| {
            s.red_modifier_scale = 0.0
        }),
        ("десатурация RRT ниже", |s| s.rrt_saturation = 0.5),
        ("десатурация RRT выше", |s| s.rrt_saturation = 1.2),
        ("десатурация выхода ниже", |s| {
            s.odt_saturation = 0.6
        }),
        ("без десатурации выхода", |s| s.odt_saturation = 1.0),
        ("окружение сильнее", |s| s.surround_gamma = 0.75),
        ("без окружения", |s| s.surround_gamma = 1.0),
    ];
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
    let default_spec = aces_spec(DisplayGamut::Srgb);
    // Две картинки сравнения: настройка может быть заметна только на одной (свечение — на тёмном насыщенном).
    let probes = [
        hue_sweep_frame(&default_spec),
        dark_saturated_frame(&default_spec),
        bright_saturated_frame(&default_spec),
    ];
    let picture_of = |spec: &ParamsSpec, params: &IspParams| -> Vec<Vec<[f32; 4]>> {
        probes
            .iter()
            .map(|raw| run_viewfinder(&context, &config, &RunRequest::whole_frame(raw, params, spec.display())).pixels)
            .collect()
    };
    let default_pictures = picture_of(&default_spec, &default_spec.build());
    for (name, edit) in edges {
        let mut settings = default;
        edit(&mut settings);
        let spec = aces_spec_with(DisplayGamut::Srgb, settings);
        let params = spec.build();
        for (scene, raw) in all_scenes(&spec) {
            assert_viewfinder_parity(&context, &format!("ACES {name} / {scene}"), &raw, &params, &spec);
        }
        assert_ne!(
            picture_of(&spec, &params),
            default_pictures,
            "{name}: настройка не дошла до картинки"
        );
    }
    assert_no_validation_errors(&context);
}
