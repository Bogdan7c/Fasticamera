//! Настоящее плоское поле основной камеры против настоящей калибровки виньетирования.
//!
//! Фикстура `flat-field-main-heldout` отложена от подгонки `device-profiles/oneplus-cph2447/lens-shading.json`
//! (в калибровку не входила), поэтому тест честно проверяет, что таблица выравнивает НЕ виденное поле.
//! Фикстура — «уменьшенный сенсор» 256×192, покрывающий все 4096×3072 активной области камеры «2»:
//! соответствие пикселей — масштабом (центр пикселя буфера → центр пикселя активной области).

#![allow(
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "интеграционный тест: битая фикстура или профиль должны ронять тест с понятным текстом"
)]

mod support;

use std::sync::Arc;

use camera_contract::fixtures::camera_id;
use camera_contract::geometry::PixelSize;
use camera_contract::lens_shading::{LensShadingCalibration, LensShadingGainGrid, ValidAreaOnSensor};
use isp_model::cfa::{BayerPattern, RgbChannel};
use isp_model::image::{CameraMosaic, RawMosaic};
use isp_model::lens_shading::{
    CorrectionStrength, LensShadingControl, LensShadingGainMap, LensShadingSampling, LensShadingStep,
    apply_lens_shading, apply_lens_shading_with,
};
use isp_model::levels::{NormalizationLevels, normalize_levels};

const ACTIVE_ARRAY: PixelSize = PixelSize::new(4096, 3072);
/// Сторона окна у угла и в центре, пиксели уменьшенного сенсора (128 пикселей активной области у угла).
const WINDOW: u32 = 8;
const TOLERANCE_RELATIVE: f64 = 0.12;
const TOLERANCE_MEAN_EV: f64 = 0.1;

/// Калибровка камеры «2» из настоящего файла профиля (минимальный разбор: без проверок приложения).
fn real_calibration() -> LensShadingCalibration {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../device-profiles/oneplus-cph2447/lens-shading.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("нет профиля {}: {error}", path.display()));
    let profile: serde_json::Value = serde_json::from_str(&text).expect("профиль — JSON");
    let camera = profile["cameras"]
        .as_array()
        .expect("cameras")
        .iter()
        .find(|camera| camera["camera_id"] == "2")
        .expect("в профиле есть камера 2");
    let grid = &camera["grid"];
    let points: Vec<[f32; 4]> = grid["points"]
        .as_array()
        .expect("points")
        .iter()
        .map(|point| {
            let values: Vec<f32> = point
                .as_array()
                .expect("точка")
                .iter()
                .map(|value| value.as_f64().expect("число") as f32)
                .collect();
            [values[0], values[1], values[2], values[3]]
        })
        .collect();
    LensShadingCalibration {
        camera: camera_id("2"),
        active_array: PixelSize::new(
            camera["active_array"]["width"].as_u64().expect("width") as u32,
            camera["active_array"]["height"].as_u64().expect("height") as u32,
        ),
        grid: LensShadingGainGrid::new(
            grid["columns"].as_u64().expect("columns") as u32,
            grid["rows"].as_u64().expect("rows") as u32,
            points,
        )
        .expect("сетка профиля валидна"),
    }
}

fn normalized_fixture() -> CameraMosaic {
    let asset = support::load_asset("flat-field-main-heldout");
    assert_eq!(asset.field("extra.cfa_arrangement").as_str(), Some("BGGR"));
    let raw = RawMosaic::new(
        PixelSize::new(asset.width, asset.height),
        BayerPattern::BGGR,
        asset.u16_samples().to_vec(),
    )
    .expect("мозаика");
    let black: Vec<f32> = asset
        .field("extra.black_level_per_position")
        .as_array()
        .expect("чёрный")
        .iter()
        .map(|value| value.as_f64().expect("число") as f32)
        .collect();
    let white = asset.field("extra.white_level").as_f64().expect("белый") as f32;
    let levels = NormalizationLevels::new([black[0], black[1], black[2], black[3]], white).expect("уровни");
    normalize_levels(&raw, &levels)
}

/// Среднее канала `channel` в квадрате `WINDOW×WINDOW` с левым верхним углом `(left, top)`.
fn window_mean(
    samples: &dyn Fn(u32, u32) -> f32,
    pattern: BayerPattern,
    channel: RgbChannel,
    left: u32,
    top: u32,
) -> f64 {
    let (mut sum, mut count) = (0.0_f64, 0_u32);
    for y in top..top + WINDOW {
        for x in left..left + WINDOW {
            if pattern.channel_at(x, y) == channel {
                sum += f64::from(samples(x, y));
                count += 1;
            }
        }
    }
    sum / f64::from(count)
}

/// Отношения «угол / центр» по четырём углам для канала.
fn corner_ratios(samples: &dyn Fn(u32, u32) -> f32, channel: RgbChannel) -> [f64; 4] {
    let (width, height) = (256, 192);
    let centre = window_mean(
        samples,
        BayerPattern::BGGR,
        channel,
        width / 2 - WINDOW / 2,
        height / 2 - WINDOW / 2,
    );
    [
        (0, 0),
        (width - WINDOW, 0),
        (0, height - WINDOW),
        (width - WINDOW, height - WINDOW),
    ]
    .map(|(left, top)| window_mean(samples, BayerPattern::BGGR, channel, left, top) / centre)
}

#[test]
fn held_out_real_flat_field_is_flattened_by_the_real_table_and_dark_without_it() {
    let mosaic = normalized_fixture();
    let calibration = real_calibration();
    let map = LensShadingGainMap::for_stream(
        &calibration,
        &ValidAreaOnSensor::whole_active_array(ACTIVE_ARRAY),
        mosaic.size(),
        CorrectionStrength::FULL,
    )
    .expect("карта потока");
    let step = LensShadingStep::decide(Some(&Arc::new(map)), LensShadingControl::default());
    let (corrected, _) = apply_lens_shading(&mosaic, &step);

    let raw_sample = |x, y| mosaic.sample(x, y);
    let corrected_sample = |x, y| corrected.sample(x, y);
    for channel in [RgbChannel::Red, RgbChannel::Green, RgbChannel::Blue] {
        let before = corner_ratios(&raw_sample, channel);
        let after = corner_ratios(&corrected_sample, channel);
        for (corner, ratio) in before.iter().enumerate() {
            assert!(
                *ratio < 0.3,
                "без коррекции угол {corner} канала {channel:?} темнее центра: {ratio:.3} (нужно < 0,3)"
            );
        }
        for (corner, ratio) in after.iter().enumerate() {
            assert!(
                (ratio - 1.0).abs() <= TOLERANCE_RELATIVE,
                "после коррекции угол {corner} канала {channel:?}: {ratio:.3} (допуск ±{TOLERANCE_RELATIVE})"
            );
        }
        let mean_ev = after.iter().map(|ratio| ratio.log2()).sum::<f64>() / 4.0;
        assert!(
            mean_ev.abs() <= TOLERANCE_MEAN_EV,
            "средний угол канала {channel:?} отстоит от центра на {mean_ev:.3} EV (допуск ±{TOLERANCE_MEAN_EV})"
        );
    }
}

/// Допуск разницы «усиление по ячейке» против «по отсчёту» на настоящей калибровке: 0,15 %. Замер: 0,123 % —
/// в самых тёмных углах, где усиление максимально и круче всего растёт (до ×5 на 128 пикселей сетки, значит
/// полпикселя сдвига даёт ~0,1 %). Это меньше шага 8-битного экрана (0,4 % у середины шкалы) и в 3 раза
/// меньше допуска коррекции ±12 %; в центре кадра разница на порядки меньше.
const PER_CELL_VERSUS_PER_SAMPLE_TOLERANCE: f64 = 1.5e-3;

#[test]
fn per_cell_sampling_differs_from_per_sample_by_a_fraction_of_a_percent_on_the_real_calibration() {
    // Поток 4:3 на весь сенсор в натуральную величину (4096×3072): ровно тот размер, на котором работает видоискатель.
    let size = ACTIVE_ARRAY;
    let map = LensShadingGainMap::for_stream(
        &real_calibration(),
        &ValidAreaOnSensor::whole_active_array(ACTIVE_ARRAY),
        size,
        CorrectionStrength::FULL,
    )
    .expect("карта потока");
    let step = LensShadingStep::decide(Some(&Arc::new(map)), LensShadingControl::default());
    // Мозаика из единиц: результат стадии — само усиление.
    let ones = CameraMosaic::from_samples(
        size,
        BayerPattern::BGGR,
        vec![1.0; size.pixel_count() as usize],
        isp_model::image::mosaic_stage::Normalized,
    )
    .expect("мозаика");
    let (per_sample, _) = apply_lens_shading_with(&ones, &step, LensShadingSampling::PerSample);
    let (per_cell, _) = apply_lens_shading_with(&ones, &step, LensShadingSampling::PerCellCentre);
    let worst = per_sample
        .samples()
        .iter()
        .zip(per_cell.samples())
        .map(|(sample, cell)| (f64::from(*cell) / f64::from(*sample) - 1.0).abs())
        .fold(0.0_f64, f64::max);
    eprintln!("макс. относительная разница по ячейке / по отсчёту на камере 2: {worst:.3e}");
    assert!(
        worst < PER_CELL_VERSUS_PER_SAMPLE_TOLERANCE,
        "разница {worst:.3e} больше допуска {PER_CELL_VERSUS_PER_SAMPLE_TOLERANCE}"
    );
    assert!(
        worst > 0.0,
        "выборки различаются: тест не должен сравнивать одно и то же"
    );
}
