//! Сквозные проверки виньетирования: синтетический кадр с реалистичной тёмной рамкой проходит
//! весь эталонный конвейер до сигнала экрана (результат доходит до конца, а не только стадия 2).

use std::sync::Arc;

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::lens_shading::{
    GAIN_CHANNELS, LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid,
};

use crate::cfa::{BayerPattern, RgbChannel};
use crate::demosaic::DemosaicMethod;
use crate::image::{DisplayEncodedImage, RawMosaic};
use crate::lens_shading::{CorrectionStrength, LensShadingControl};
use crate::params::{FrameInputs, IspParams, ProcessingControls};
use crate::pipeline::run_reference;
use crate::sensor::SensorDescription;
use crate::statistics::{AwbStatisticsInput, AwbStatisticsSettings, TileGrid, compute_awb_statistics};
use crate::test_support::{
    LAMP_NEUTRAL, MAIN_ACTIVE_ARRAY, calibrated_passport, lamp_neutral, main_camera_calibration,
};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::CaptureColorHints;

/// Буфер — сенсор, смотящий через биннинг 16×: 256×192 покрывает все 4096×3072.
const BUFFER: PixelSize = PixelSize::new(256, 192);
const BINNING: f64 = 16.0;
const BLACK: u32 = 64;
const WHITE: u32 = 4095;
/// Сила виньетирования по каналам `[R, G_even, G_odd, B]`: в углу усиление `1 + k` (до ~3 EV).
const VIGNETTE_K: [f64; GAIN_CHANNELS] = [5.0, 4.2, 4.5, 6.0];

/// Радиальное усиление канала в пикселе `(x, y)` активной области. Центр сдвинут (0,45; 0,55), чтобы
/// тёмные углы были разными и перестановка осей ломала тест.
fn vignette_gain(channel: usize, x: f64, y: f64) -> f64 {
    let u = (x - 0.45 * f64::from(MAIN_ACTIVE_ARRAY.width)) / (0.55 * f64::from(MAIN_ACTIVE_ARRAY.width));
    let v = (y - 0.55 * f64::from(MAIN_ACTIVE_ARRAY.height)) / (0.55 * f64::from(MAIN_ACTIVE_ARRAY.height));
    1.0 + VIGNETTE_K[channel] * (u * u + v * v) / 2.0
}

/// Калибровка камеры, снятая на этом поле (узлы сетки лежат на нём).
fn radial_calibration() -> LensShadingCalibration {
    let (columns, rows) = (33_u32, 25_u32);
    let mut points = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let x = f64::from(MAIN_ACTIVE_ARRAY.width - 1) * f64::from(column) / f64::from(columns - 1);
            let y = f64::from(MAIN_ACTIVE_ARRAY.height - 1) * f64::from(row) / f64::from(rows - 1);
            let mut gains = [1.0_f32; GAIN_CHANNELS];
            for (channel, gain) in gains.iter_mut().enumerate() {
                *gain = vignette_gain(channel, x, y) as f32;
            }
            points.push(gains);
        }
    }
    LensShadingCalibration {
        camera: camera_id("2"),
        active_array: MAIN_ACTIVE_ARRAY,
        grid: LensShadingGainGrid::new(columns, rows, points).unwrap(),
    }
}

fn passport() -> StreamPassport {
    let mut passport = calibrated_passport(
        BUFFER,
        PixelRect {
            left: 0,
            top: 0,
            size: MAIN_ACTIVE_ARRAY,
        },
    );
    passport.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(radial_calibration()));
    passport.static_levels.black_level_pattern = [BLACK; 4];
    passport.static_levels.white_level = WHITE;
    passport
}

/// Серый кадр сцены яркости `t` (в единицах белого сенсора для нейтрального канала), снятый через
/// виньетирование: каждый канал делится на своё усиление (независимо от кода ISP).
fn vignetted_gray_frame(pattern: BayerPattern) -> RawMosaic {
    let mut samples = Vec::new();
    for y in 0..BUFFER.height {
        for x in 0..BUFFER.width {
            let (channel, neutral) = match pattern.channel_at(x, y) {
                RgbChannel::Red => (0, LAMP_NEUTRAL[0]),
                RgbChannel::Green if y % 2 == 0 => (1, LAMP_NEUTRAL[1]),
                RgbChannel::Green => (2, LAMP_NEUTRAL[1]),
                RgbChannel::Blue => (3, LAMP_NEUTRAL[2]),
            };
            // Центр пикселя буфера в пикселях активной области (биннинг 16×).
            let (active_x, active_y) = (
                (f64::from(x) + 0.5) * BINNING - 0.5,
                (f64::from(y) + 0.5) * BINNING - 0.5,
            );
            let signal = 0.5 * neutral / vignette_gain(channel, active_x, active_y);
            let raw = f64::from(BLACK) + signal * f64::from(WHITE - BLACK);
            samples.push(raw.round() as u16);
        }
    }
    RawMosaic::new(BUFFER, pattern, samples).unwrap()
}

fn params(sensor: &SensorDescription, control: LensShadingControl) -> IspParams {
    let frame = FrameInputs::from_capture_hints(sensor, &CaptureColorHints::default()).unwrap();
    let controls = ProcessingControls {
        lens_shading: control,
        ..ProcessingControls::with_defaults(
            lamp_neutral(),
            DemosaicMethod::Preview,
            crate::image::DisplayGamut::Srgb,
        )
    };
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

/// Яркость (Rec. 709 по кодированным значениям) среднего по окну 4×4 вокруг `(centre_x, centre_y)`.
fn patch_luminance(image: &DisplayEncodedImage, centre_x: u32, centre_y: u32) -> f32 {
    let mut sum = 0.0;
    for y in centre_y - 2..centre_y + 2 {
        for x in centre_x - 2..centre_x + 2 {
            let [r, g, b] = image.pixel(x, y);
            sum += 0.2126 * r + 0.7152 * g + 0.0722 * b;
        }
    }
    sum / 16.0
}

/// Яркость в центре и в четырёх углах выхода (окна не касаются края демозаика).
fn centre_and_corners(image: &DisplayEncodedImage) -> (f32, [f32; 4]) {
    let size = image.size();
    let margin = 6;
    let centre = patch_luminance(image, size.width / 2, size.height / 2);
    let corners = [
        patch_luminance(image, margin, margin),
        patch_luminance(image, size.width - margin, margin),
        patch_luminance(image, margin, size.height - margin),
        patch_luminance(image, size.width - margin, size.height - margin),
    ];
    (centre, corners)
}

#[test]
fn vignetted_frame_comes_out_flat_with_the_stage_on_and_dark_with_it_off() {
    let sensor = SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap();
    let raw = vignetted_gray_frame(sensor.pattern());

    let corrected = run_reference(&raw, &params(&sensor, LensShadingControl::default())).unwrap();
    let (centre, corners) = centre_and_corners(&corrected.display_encoded);
    assert!(centre > 0.1, "кадр не чёрный: {centre}");
    for corner in corners {
        assert!(
            (corner / centre - 1.0).abs() < 0.01,
            "угол {corner} против центра {centre} (с коррекцией должно быть в пределах 1 %)"
        );
    }

    let uncorrected = run_reference(&raw, &params(&sensor, LensShadingControl::Disabled)).unwrap();
    let (centre, corners) = centre_and_corners(&uncorrected.display_encoded);
    for corner in corners {
        assert!(
            corner < 0.85 * centre,
            "без коррекции углы должны быть заметно темнее: {corner} против {centre}"
        );
    }
}

#[test]
fn half_strength_lands_between_the_corrected_and_the_uncorrected_corner() {
    let sensor = SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap();
    let raw = vignetted_gray_frame(sensor.pattern());
    let corner_ratio = |control| {
        let run = run_reference(&raw, &params(&sensor, control)).unwrap();
        let (centre, corners) = centre_and_corners(&run.display_encoded);
        corners[3] / centre
    };
    let full = corner_ratio(LensShadingControl::default());
    let half = corner_ratio(LensShadingControl::Enabled(CorrectionStrength::new(0.5).unwrap()));
    let off = corner_ratio(LensShadingControl::Disabled);
    assert!(off < half && half < full, "{off} < {half} < {full}");
}

#[test]
fn awb_statistics_after_shading_are_flat_across_a_vignetted_frame() {
    let sensor = SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap();
    let raw = vignetted_gray_frame(sensor.pattern());
    let settings = AwbStatisticsSettings {
        grid: TileGrid { columns: 4, rows: 3 },
        saturation_threshold: 0.95,
        sum_headroom: 16.0,
    };
    let mean_green = |control| {
        let run = run_reference(&raw, &params(&sensor, control)).unwrap();
        let statistics = compute_awb_statistics(
            AwbStatisticsInput::new(&run.normalized, &run.shading_corrected).unwrap(),
            &settings,
        )
        .unwrap();
        let tiles = statistics.tiles();
        let per_tile: Vec<f64> = tiles
            .iter()
            .map(|tile| f64::from(tile.sum_g) / f64::from(tile.used_cells.max(1)))
            .collect();
        let max = per_tile.iter().copied().fold(f64::MIN, f64::max);
        let min = per_tile.iter().copied().fold(f64::MAX, f64::min);
        (min, max)
    };
    let (min, max) = mean_green(LensShadingControl::default());
    assert!(max / min < 1.03, "после коррекции плитки равны: {min}..{max}");
    let (min, max) = mean_green(LensShadingControl::Disabled);
    assert!(max / min > 1.5, "без коррекции плитки сильно разные: {min}..{max}");
}

#[test]
fn stage_two_output_feeds_statistics_not_the_raw_levels_stage() {
    // Статистика берёт мозаику после стадии 2: тип `ShadingCorrectedMosaic` не даёт подать другую.
    let sensor = SensorDescription::from_stream_passport(&passport(), &main_camera_calibration()).unwrap();
    let raw = vignetted_gray_frame(sensor.pattern());
    let run = run_reference(&raw, &params(&sensor, LensShadingControl::default())).unwrap();
    assert_ne!(run.normalized.samples(), run.shading_corrected.samples());
}
