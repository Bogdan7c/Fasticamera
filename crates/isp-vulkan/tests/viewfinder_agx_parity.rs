//! Паритет ISP видоискателя на GPU (RADV, validation) с эталоном `isp-model` (Preview) для основы AgX и
//! своего белого точки (решения 176, 177): те же допуски, что у прежней основы (`viewfinder_parity.rs`):
//! max |Δ| ≤ ½ LSB 10-битного выхода + 1e-5, ноль пикселей хуже 1 LSB.
//!
//! Сцены: синтетика (серая шкала, полосы, пересветы, около чёрного), реальные кропы, кадр с картой
//! виньетирования и яркими пятнами в углу (там свой белый точки заметно выше 1), обе цели вывода
//! (Display P3 и sRGB). Отдельно: переключение «старая основа → AgX → старая» на одном ISP.

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
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::working_io::WorkingInputGamut;
use isp_vulkan::viewfinder::ViewfinderStorageFormat;
use lens_shading_support::{asymmetric_map, with_map};
use viewfinder_support::{
    HEIGHT_OF_SYNTHETIC_SCENES as HEIGHT, ParamsSpec, RawFrame, RunRequest, WIDTH_OF_SYNTHETIC_SCENES as WIDTH,
    assert_no_validation_errors, assert_viewfinder_parity, gpu_context_or_skip, isp_support, parity_with_pixels,
    reference_upright_encoded, run_viewfinder, synthetic_scenes, test_config,
};

fn agx_spec(gamut: DisplayGamut) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: ToneBase::Agx(AgxSettings::default()),
        ..ParamsSpec::default()
    }
}

/// AgX с мягким сжатием охвата на входе (второй вариант входа: у настоящей камеры цвета внутри Rec. 2020 и
/// обрезка нулём ничего не меняет, а сжатие меняет цвета у границы охвата).
fn agx_soft_input_spec(gamut: DisplayGamut) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: ToneBase::Agx(AgxSettings {
            input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
            ..AgxSettings::default()
        }),
        ..ParamsSpec::default()
    }
}

#[test]
fn synthetic_scenes_match_reference_for_both_outputs() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
        let spec = agx_spec(gamut);
        let params = spec.build();
        assert!(matches!(
            params.tone_base,
            isp_model::tone::base::ResolvedToneBase::Agx(_)
        ));
        for (name, raw) in synthetic_scenes(&spec) {
            let (half, ten_bit) =
                assert_viewfinder_parity(&context, &format!("AgX {gamut:?} {name}"), &raw, &params, &spec);
            eprintln!("AgX {gamut:?} {name}: max |Δ| fp16 {half:.3e}, 10 бит {ten_bit:.3e}");
        }
    }
    // Мягкое сжатие охвата на входе: паритет и отличие от обрезки нулём на насыщенных полосах.
    let soft = agx_soft_input_spec(DisplayGamut::Srgb);
    let soft_params = soft.build();
    for (name, raw) in synthetic_scenes(&soft) {
        assert_viewfinder_parity(&context, &format!("AgX вход-сжатие {name}"), &raw, &soft_params, &soft);
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
        let controls = ProcessingControls {
            tone_base: ToneBase::Agx(AgxSettings::default()),
            ..ProcessingControls::with_defaults(capture.neutral, DemosaicMethod::Preview, DisplayGamut::DisplayP3)
        };
        let params = IspParams::resolve(&capture.sensor, &frame, &controls).unwrap();
        let raw = RawFrame {
            width: capture.raw.size().width,
            height: capture.raw.size().height,
            pattern: capture.raw.pattern(),
            samples: capture.raw.samples().to_vec(),
        };
        assert_viewfinder_parity(&context, crop, &raw, &params, &agx_spec(DisplayGamut::DisplayP3));
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
        let spec = agx_spec(gamut);
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
                *value >= 0.995,
                "{gamut:?}: угловое пятно должно быть белым, канал {channel} = {value}"
            );
        }
        let background = run.pixel(3, 3);
        assert!(
            background[1] < 0.6,
            "{gamut:?}: серый фон не должен быть белым: {background:?}"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn one_isp_switches_between_bases_and_each_matches_its_own_reference() {
    let Some(context) = gpu_context_or_skip() else { return };
    let legacy_spec = ParamsSpec::default();
    let agx = agx_spec(DisplayGamut::DisplayP3);
    let map = asymmetric_map(5, 4);
    let legacy_params = with_map(&legacy_spec.build(), &map);
    let agx_params = with_map(&agx.build(), &map);
    let raw = corner_spot_frame(&agx);
    let config = test_config(ViewfinderStorageFormat::HalfFloatOnly);

    let mut isp = isp_vulkan::viewfinder::ViewfinderIsp::new(&context, &config).unwrap();
    let source = viewfinder_support::GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = source.source();
    let mut runs = Vec::new();
    for (frame, params) in [&legacy_params, &agx_params, &legacy_params].into_iter().enumerate() {
        let request = RunRequest::whole_frame(&raw, params, agx.display());
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
    // Картинки двух основ различаются (иначе переключение ничего не доказывает).
    assert_ne!(runs[0].pixels, runs[1].pixels, "основы дали одну картинку");
    // Статистика AE/AWB меряет честную базу до основы тона (решение 175): от основы не зависит.
    assert!(runs[0].report.awb_statistics.is_some());
    assert_eq!(
        runs[0].report.awb_statistics, runs[1].report.awb_statistics,
        "статистика зависит от основы"
    );
    assert_eq!(
        runs[0].pixels, runs[2].pixels,
        "возврат на прежнюю основу — та же картинка бит в бит"
    );
    for (run, params) in runs.iter().zip([&legacy_params, &agx_params, &legacy_params]) {
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
        2,
        "по варианту на основу, без пересоздания"
    );
    assert_no_validation_errors(&context);
}

/// У основы AgX каждая стадия-выключатель вырезается и меняет картинку (кроме статистики AWB, которая картинку
/// не трогает): набор стадий действует и на этом варианте шейдера, а не только у прежней основы.
#[test]
fn every_stage_off_changes_the_agx_picture() {
    use isp_vulkan::viewfinder::{IspStage, IspStageSet};
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = agx_soft_input_spec(DisplayGamut::Srgb);
    // Вид с цветом и таблицей: иначе стадии вида нечего резать (при нейтральном виде они пусты).
    let look = look_support::with_lut(look_support::saturation_vibrance(0.6, 0.3), look_support::known_lut());
    let params = with_map(&spec.build_with_look(look), &asymmetric_map(5, 4));
    // Насыщенные полосы с яркостью от почти чёрного до пересвета: вход, кривая и выходное сжатие здесь работают.
    let bars =
        isp_model::test_patterns::color_bars(camera_contract::geometry::PixelSize::new(WIDTH, HEIGHT), 1.0).unwrap();
    let raw = RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        f64::from(bars.pixel(x, y)[channel]) * (0.02 + 1.5 * f64::from(y) / f64::from(HEIGHT - 1))
    });
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
                "без {stage:?} картинка AgX обязана измениться"
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

/// Кадр: полезная область со световой «шашкой» и градиентом (сдвиг на пиксель меняет значения), паддинг — насыщение.
/// В правом нижнем углу полезной области (x ≥ 50, y ≥ 36) — яркое пятно: сенсор там насыщен, а карта виньетирования
/// усиливает угол сильнее всего, поэтому свой белый точки там заметно выше 1.
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

/// Кадрирование внутри полезной области со смещением и каждым поворотом: основа AgX берёт карту виньетирования и
/// белый точки по координатам ПОЛЕЗНОЙ области (а не кропа и не буфера), как и прежняя основа.
#[test]
fn crop_inside_offset_valid_area_and_every_rotation_match_the_agx_reference() {
    use camera_contract::geometry::{PixelRect, PixelSize};
    use viewfinder_support::{SourceLayout, extent, parity_with_pixels, reference_upright_encoded_in_valid_area};
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = agx_spec(DisplayGamut::DisplayP3);
    let looked = spec.build_with_look(look_support::saturation_vibrance(0.5, 0.2));
    for params in [
        with_map(&spec.build(), &asymmetric_map(6, 5)),
        with_map(&looked, &asymmetric_map(6, 5)),
    ] {
        let (raw, valid) = padded_buffer_frame(&spec);
        // Кадрирование НЕ в начале полезной области и накрывает яркое пятно угла: в её координатах — (24, 14), размер
        // 40×34 (до правого и нижнего краёв). Свой белый и карта берутся по координатам полезной области.
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
    }
    assert_no_validation_errors(&context);
}

fn agx_hue_spec(gamut: DisplayGamut, degree: f32) -> ParamsSpec {
    ParamsSpec {
        gamut,
        tone_base: ToneBase::Agx(AgxSettings {
            hue_restoration: degree,
            ..AgxSettings::default()
        }),
        ..ParamsSpec::default()
    }
}

/// Развёртка по оттенку (насыщенные цвета всех оттенков, от тёмных к светлым): возврат оттенка здесь работает.
fn hue_sweep_frame(spec: &ParamsSpec) -> RawFrame {
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |x, y, channel| {
        look_support::hue_sweep_light(x, y, WIDTH, HEIGHT, channel, spec.neutral)
    })
}

/// Возврат оттенка (решение 180) на GPU: степени 0, 0,5 и 1 совпадают с эталоном на всех сценах и обеих целях, и
/// каждая степень даёт СВОЮ картинку (иначе шаг на GPU не работал бы или не зависел от степени).
#[test]
fn hue_restoration_degrees_match_reference_and_each_changes_the_picture() {
    let Some(context) = gpu_context_or_skip() else { return };
    for gamut in [DisplayGamut::DisplayP3, DisplayGamut::Srgb] {
        let mut pictures: Vec<Vec<[f32; 4]>> = Vec::new();
        for degree in [0.0, 0.5, 1.0] {
            let spec = agx_hue_spec(gamut, degree);
            let params = spec.build();
            let mut scenes = synthetic_scenes(&spec);
            scenes.push(("развёртка по оттенку", hue_sweep_frame(&spec)));
            for (name, raw) in &scenes {
                let (half, ten_bit) = assert_viewfinder_parity(
                    &context,
                    &format!("возврат оттенка {degree} / {gamut:?} / {name}"),
                    raw,
                    &params,
                    &spec,
                );
                eprintln!("возврат оттенка {degree} {gamut:?} {name}: max |Δ| fp16 {half:.3e}, 10 бит {ten_bit:.3e}");
            }
            let sweep = run_viewfinder(
                &context,
                &test_config(ViewfinderStorageFormat::HalfFloatOnly),
                &RunRequest::whole_frame(&scenes[4].1, &params, spec.display()),
            );
            pictures.push(sweep.pixels);
        }
        assert_ne!(pictures[0], pictures[1], "{gamut:?}: степень 0,5 не отличается от 0");
        assert_ne!(pictures[1], pictures[2], "{gamut:?}: степень 1 не отличается от 0,5");
        assert_ne!(pictures[0], pictures[2], "{gamut:?}: степень 1 не отличается от 0");
    }
    assert_no_validation_errors(&context);
}
