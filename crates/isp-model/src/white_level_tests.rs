//! Свой белый у каждой точки (решение 177): формула, выборка, вырезки и сквозной кадр с пятнами в углу.

use camera_contract::geometry::PixelSize;

use crate::cfa::{BayerPattern, RgbChannel};
use crate::color::dng::CameraNeutral;
use crate::demosaic::DemosaicMethod;
use crate::image::{DisplayGamut, RawMosaic, ShadingCorrectedMosaic, mosaic_stage};
use crate::lens_shading::{CropInValidArea, LensShadingControl, LensShadingSampling, LensShadingStep};
use crate::params::{FrameInputs, IspParams, ProcessingControls};
use crate::pipeline::{run_reference, run_reference_working};
use crate::sensor::SensorDescription;
use crate::shading_pipeline_tests::{BINNING, BLACK, BUFFER, WHITE, passport, vignette_gain};
use crate::test_support::{LAMP_NEUTRAL, lamp_neutral, main_camera_calibration};
use crate::tone::agx_settings::AgxSettings;
use crate::tone::base::{ToneBase, WhitePolicy};
use crate::white_balance::{WhiteBalanceGains, apply_white_balance};
use crate::white_level::{WhiteLevelField, WhiteLevelPlane, apply_white_balance_per_point};
use camera_contract::frame_passport::CaptureColorHints;

fn sensor() -> SensorDescription {
    SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap()
}

fn agx_params(sensor: &SensorDescription, control: LensShadingControl, demosaic: DemosaicMethod) -> IspParams {
    let frame = FrameInputs::from_capture_hints(sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls {
        lens_shading: control,
        ..ProcessingControls::with_defaults(lamp_neutral(), demosaic, DisplayGamut::Srgb).with_legacy_tone()
    };
    IspParams::resolve_with_base(sensor, &frame, &controls, &ToneBase::Agx(AgxSettings::default())).unwrap()
}

fn lamp_gains() -> WhiteBalanceGains {
    WhiteBalanceGains::from_camera_neutral(lamp_neutral())
}

#[test]
fn white_level_is_exactly_one_without_correction() {
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::Disabled, DemosaicMethod::Quality);
    assert!(matches!(params.lens_shading, LensShadingStep::Skip(_)));
    let field = WhiteLevelField::new(
        &params.lens_shading,
        &params.white_balance,
        CropInValidArea::whole(BUFFER),
        LensShadingSampling::PerSample,
    );
    for (x, y) in [(0, 0), (17, 3), (255, 191)] {
        assert_eq!(field.level_at(x, y), 1.0);
    }
}

/// Усиления кадра виньетирования (независимый оракул), `[R, G_even, G_odd, B]` в точке мозаики `(x, y)`.
fn oracle_level(x: u32, y: u32) -> f32 {
    let active_x = (f64::from(x) + 0.5) * BINNING - 0.5;
    let active_y = (f64::from(y) + 0.5) * BINNING - 0.5;
    let balance = lamp_gains().as_array().map(f64::from);
    let products = [
        vignette_gain(0, active_x, active_y) * balance[0],
        vignette_gain(1, active_x, active_y) * balance[1],
        vignette_gain(2, active_x, active_y) * balance[1],
        vignette_gain(3, active_x, active_y) * balance[2],
    ];
    products.into_iter().fold(f64::MAX, f64::min) as f32
}

#[test]
fn white_level_is_the_smallest_corrected_channel_saturation_and_grows_toward_the_corner() {
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Quality);
    let field = WhiteLevelField::new(
        &params.lens_shading,
        &params.white_balance,
        CropInValidArea::whole(BUFFER),
        LensShadingSampling::PerSample,
    );
    for (x, y) in [(0, 0), (255, 0), (0, 191), (255, 191), (115, 105), (60, 40)] {
        let expected = oracle_level(x, y);
        let actual = field.level_at(x, y);
        // Сетка 33×25 приближает квадратичное поле билинейно: расхождение до ~0,3 %.
        assert!(
            (actual / expected - 1.0).abs() < 4e-3,
            "({x}, {y}): {actual} против оракула {expected}"
        );
    }
    let centre = field.level_at(115, 105);
    let corner = field.level_at(0, 0);
    assert!((1.0..1.3).contains(&centre), "в центре почти 1: {centre}");
    assert!(
        corner > 3.5 * centre,
        "в углу белый вырастает: {corner} против {centre}"
    );
}

#[test]
fn preview_samples_the_cell_centre_while_quality_samples_each_sample() {
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Preview);
    let cell = |sampling| {
        WhiteLevelField::new(
            &params.lens_shading,
            &params.white_balance,
            CropInValidArea::whole(BUFFER),
            sampling,
        )
    };
    let per_cell = cell(LensShadingSampling::PerCellCentre);
    let per_sample = cell(LensShadingSampling::PerSample);
    // Все четыре отсчёта ячейки видят один белый; поэтапно по отсчёту белый различается внутри ячейки.
    for (x0, y0) in [(0_u32, 0_u32), (100, 60), (250, 186)] {
        let anchor = per_cell.level_at(x0, y0);
        for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
            assert_eq!(per_cell.level_at(x0 + dx, y0 + dy), anchor);
        }
    }
    assert_ne!(per_sample.level_at(0, 0), per_sample.level_at(1, 1));
    // Плоскость Preview — половинное разрешение, значение ячейки.
    let plane = WhiteLevelPlane::for_demosaic(&per_cell, DemosaicMethod::Preview, BUFFER);
    assert_eq!(plane.size(), PixelSize::new(BUFFER.width / 2, BUFFER.height / 2));
    assert_eq!(plane.level(50, 30), per_cell.level_at(100, 60));
    // Плоскость Quality — полное разрешение, белый точки пикселя.
    let plane = WhiteLevelPlane::for_demosaic(&per_sample, DemosaicMethod::Quality, BUFFER);
    assert_eq!(plane.size(), BUFFER);
    assert_eq!(plane.level(101, 61), per_sample.level_at(101, 61));
}

#[test]
fn crop_field_equals_the_whole_frame_field_shifted_by_the_crop_origin() {
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Quality);
    let whole = WhiteLevelField::new(
        &params.lens_shading,
        &params.white_balance,
        CropInValidArea::whole(BUFFER),
        LensShadingSampling::PerSample,
    );
    let crop = WhiteLevelField::new(
        &params.lens_shading,
        &params.white_balance,
        CropInValidArea::at(64, 32, BUFFER).unwrap(),
        LensShadingSampling::PerSample,
    );
    for (x, y) in [(0_u32, 0_u32), (5, 9), (100, 50)] {
        assert_eq!(crop.level_at(x, y).to_bits(), whole.level_at(64 + x, 32 + y).to_bits());
    }
}

#[test]
fn per_point_clip_caps_at_the_local_white_where_the_sensor_clip_caps_at_one() {
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Quality);
    let field = WhiteLevelField::new(
        &params.lens_shading,
        &params.white_balance,
        CropInValidArea::whole(BUFFER),
        LensShadingSampling::PerSample,
    );
    let samples = vec![50.0_f32; (BUFFER.width * BUFFER.height) as usize];
    let mosaic =
        ShadingCorrectedMosaic::from_samples(BUFFER, BayerPattern::BGGR, samples, mosaic_stage::ShadingCorrected)
            .unwrap();
    let per_point = apply_white_balance_per_point(&mosaic, &params.white_balance, &field);
    let sensor_clip = apply_white_balance(&mosaic, &params.white_balance);
    for (x, y) in [(0_u32, 0_u32), (255, 191), (115, 105), (7, 120)] {
        assert_eq!(per_point.sample(x, y), field.level_at(x, y), "({x}, {y})");
        assert_eq!(sensor_clip.sample(x, y), 1.0);
    }
    // Ниже своего белого отсчёт не трогается: 0,5 после баланса остаётся 0,5·wb.
    let mild = ShadingCorrectedMosaic::from_samples(
        BUFFER,
        BayerPattern::BGGR,
        vec![0.5; (BUFFER.width * BUFFER.height) as usize],
        mosaic_stage::ShadingCorrected,
    )
    .unwrap();
    let kept = apply_white_balance_per_point(&mild, &params.white_balance, &field);
    let expected = 0.5 * params.white_balance.gain(RgbChannel::Green);
    assert_eq!(kept.sample(1, 0), expected.min(field.level_at(1, 0)));
}

/// Кадр: серый фон и квадратные пятна сцены яркости `t` (в единицах белого сенсора; баланс по нейтрали лампы)
/// в заданных клетках мозаики `[x0, y0, размер]`, снятые через то же виньетирование; сенсор насыщается на 1,0.
fn frame_with_patches(pattern: BayerPattern, patches: &[([u32; 3], f64)]) -> RawMosaic {
    let mut samples = Vec::new();
    for y in 0..BUFFER.height {
        for x in 0..BUFFER.width {
            let (channel, neutral) = match pattern.channel_at(x, y) {
                RgbChannel::Red => (0, LAMP_NEUTRAL[0]),
                RgbChannel::Green if y % 2 == 0 => (1, LAMP_NEUTRAL[1]),
                RgbChannel::Green => (2, LAMP_NEUTRAL[1]),
                RgbChannel::Blue => (3, LAMP_NEUTRAL[2]),
            };
            let brightness = patches
                .iter()
                .find(|([x0, y0, size], _)| x >= *x0 && x < x0 + size && y >= *y0 && y < y0 + size)
                .map_or(0.4, |(_, t)| *t);
            let (active_x, active_y) = (
                (f64::from(x) + 0.5) * BINNING - 0.5,
                (f64::from(y) + 0.5) * BINNING - 0.5,
            );
            let signal = (brightness * neutral / vignette_gain(channel, active_x, active_y)).min(1.0);
            samples.push((f64::from(BLACK) + signal * f64::from(WHITE - BLACK)).round() as u16);
        }
    }
    RawMosaic::new(BUFFER, pattern, samples).unwrap()
}

/// Закодированная яркость пятна в Preview: центр клетки пятна `[x0, y0, размер]` (пиксель демозаика — ячейка 2×2).
fn patch_value(image: &crate::image::DisplayEncodedImage, patch: [u32; 3]) -> [f32; 3] {
    image.pixel(patch[0] / 2 + patch[2] / 4, patch[1] / 2 + patch[2] / 4)
}

const MILD: [u32; 3] = [4, 4, 16];
const BRIGHT: [u32; 3] = [4, 36, 16];
const BLOWN: [u32; 3] = [40, 4, 16];

fn patches() -> [([u32; 3], f64); 3] {
    // Яркость сцены: 1,2 и 2,0 ниже белого этого угла (≈ 4), 20 — настоящий пересвет (сенсор насыщен).
    [(MILD, 1.2), (BRIGHT, 2.0), (BLOWN, 20.0)]
}

#[test]
fn corner_highlights_keep_their_gradations_and_true_overexposure_is_pure_white() {
    let sensor = sensor();
    let raw = frame_with_patches(sensor.pattern(), &patches());
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Preview);
    assert_eq!(params.white_policy, WhitePolicy::PerPoint);
    let run = run_reference_working(&raw, &params).unwrap();
    let mild = patch_value(&run.display_encoded, MILD);
    let bright = patch_value(&run.display_encoded, BRIGHT);
    let blown = patch_value(&run.display_encoded, BLOWN);
    // Градации ниже своего белого сохранены: 1,2 < 2,0 < белого, и оба заметно ниже единицы.
    assert!(
        mild[1] < bright[1] - 0.01 && bright[1] < 0.995,
        "mild {mild:?}, bright {bright:?}"
    );
    assert!(mild[1] > 0.5, "пятно светлое: {mild:?}");
    // Настоящий пересвет — белый, а не розовый: все каналы 1 (допуск f32-матриц).
    for channel in blown {
        assert!(channel > 0.998, "пересвет не белый: {blown:?}");
    }
    // Пятно уложилось в карту белого: белый плоскости в углу заметно выше 1.
    assert!(run.white_levels.level(10, 10) > 3.0);

    // Прежняя основа режет те же пятна по 1,0: умеренные пятна сливаются в белое, градации потеряны.
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb)
        .with_legacy_tone();
    let legacy_params = IspParams::resolve(&sensor, &frame, &controls).unwrap();
    assert_eq!(legacy_params.white_policy, WhitePolicy::SensorLevel);
    let legacy = run_reference(&raw, &legacy_params).unwrap();
    let legacy_mild = patch_value(&legacy.display_encoded, MILD);
    let legacy_bright = patch_value(&legacy.display_encoded, BRIGHT);
    assert!(
        (legacy_mild[1] - legacy_bright[1]).abs() < 0.005,
        "{legacy_mild:?} {legacy_bright:?}"
    );
    assert!(legacy_bright[1] > 0.99);
}

#[test]
fn without_correction_the_point_white_is_one_and_the_image_clips_at_the_sensor_white() {
    let sensor = sensor();
    let raw = frame_with_patches(sensor.pattern(), &patches());
    let params = agx_params(&sensor, LensShadingControl::Disabled, DemosaicMethod::Preview);
    let run = run_reference_working(&raw, &params).unwrap();
    assert!(run.white_levels.levels().iter().all(|level| *level == 1.0));
    // Без коррекции уровень отсчёта насыщения — 1: после баланса сцена выше 1 обрезана до 1.
    assert!(
        run.front_end
            .white_balanced
            .samples()
            .iter()
            .all(|sample| *sample <= 1.0)
    );
}

#[test]
fn legacy_params_are_unchanged_by_the_new_base_machinery() {
    // Прежний вход (`resolve`) и явный выбор Legacy дают те же параметры и тот же кадр бит в бит.
    let sensor = sensor();
    let raw = frame_with_patches(sensor.pattern(), &patches());
    let frame = FrameInputs::from_capture_hints(&sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls::with_defaults(lamp_neutral(), DemosaicMethod::Preview, DisplayGamut::Srgb)
        .with_legacy_tone();
    let plain = IspParams::resolve(&sensor, &frame, &controls).unwrap();
    let explicit = IspParams::resolve_with_base(&sensor, &frame, &controls, &ToneBase::Legacy).unwrap();
    assert_eq!(plain, explicit);
    let first = run_reference(&raw, &plain).unwrap();
    // Старая основа: обрезка по SENSOR_CLIP_LEVEL, как до 14a.
    assert!(first.white_balanced.samples().iter().all(|sample| *sample <= 1.0));
    assert!(
        first
            .display_encoded
            .pixels()
            .iter()
            .flatten()
            .all(|c| (0.0..=1.0).contains(c))
    );
    // Каждая основа отказывается считаться чужим входом (различимая ошибка).
    let agx = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Preview);
    assert!(run_reference(&raw, &agx).is_err());
    assert!(run_reference_working(&raw, &plain).is_err());
}

#[test]
fn neutral_of_the_lamp_has_the_smallest_balance_gain_of_exactly_one() {
    // Основа формулы «без коррекции L = 1»: наименьшее усиление баланса ровно 1 при любой нейтрали.
    for neutral in [LAMP_NEUTRAL, [0.3, 0.7, 1.0], [1.0, 0.45, 0.6]] {
        let gains = WhiteBalanceGains::from_camera_neutral(CameraNeutral::new(neutral).unwrap()).as_array();
        assert_eq!(gains.iter().copied().fold(f32::MAX, f32::min), 1.0);
    }
}

#[test]
fn camera_white_maps_to_the_same_grey_in_the_working_space() {
    // Белый камеры (L, L, L) после баланса → XYZ D50 (матрица DNG) → Брэдфорд D50 → D65 и Rec. 2020 → (L, L, L):
    // Y = L сохраняется, цветности нет. Допуск — округление f32 матриц (≈ 1e-6 относительно).
    let sensor = sensor();
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Quality);
    let crate::tone::base::ResolvedToneBase::Agx(base) = &params.tone_base else {
        panic!("основа AgX");
    };
    for level in [1.0_f32, 2.7, 6.0] {
        let xyz = params.color.camera_to_xyz_d50().apply([level; 3]);
        let working = base.xyz_d50_to_working(xyz);
        for channel in working {
            assert!((channel / level - 1.0).abs() < 3e-6, "L = {level}: {working:?}");
        }
    }
}

#[test]
fn vignetted_grey_frame_renders_flat_and_neutral_with_agx() {
    // Сквозь основу AgX и своё белое серая сцена с виньетированием остаётся серой и ровной от центра к углам.
    let sensor = sensor();
    let raw = frame_with_patches(sensor.pattern(), &[]);
    let params = agx_params(&sensor, LensShadingControl::default(), DemosaicMethod::Preview);
    let run = run_reference_working(&raw, &params).unwrap();
    let encoded = &run.display_encoded;
    let centre = encoded.pixel(64, 48);
    let corner = encoded.pixel(6, 6);
    for pixel in [centre, corner] {
        assert!(
            pixel[0].max(pixel[1]).max(pixel[2]) - pixel[0].min(pixel[1]).min(pixel[2]) < 0.01,
            "{pixel:?}"
        );
    }
    assert!(centre[1] > 0.2, "кадр не чёрный: {centre:?}");
    assert!(
        (corner[1] / centre[1] - 1.0).abs() < 0.02,
        "угол {corner:?} против центра {centre:?}"
    );
}
