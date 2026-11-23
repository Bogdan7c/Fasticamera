//! Вектороскоп: оси Y'CbCr BT.709 и OKLab a/b.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{code_of, flat, picture};
use isp_model::color::chromaticity::Primaries;
use isp_model::color::oklab::xyz_d65_to_oklab;
use isp_model::image::DisplayGamut;
use scope_model::oklab_rule::OklabVectorscopeRule;
use scope_model::vectorscope_ycbcr::YCbCrVectorscopeRule;
use scope_model::{SamplePosition, SampleStride, accumulate, hits_of_code};

const SIZE: u32 = 256;

fn position() -> SamplePosition {
    SamplePosition { x: 0, picture_width: 1 }
}

/// Шесть цветов полос 75 %: код канала = round(0,75 · 1023) там, где канал включён.
fn bars_75() -> [[u16; 3]; 6] {
    let on = code_of(0.75);
    [
        [on, 0, 0],
        [0, on, 0],
        [0, 0, on],
        [0, on, on],
        [on, 0, on],
        [on, on, 0],
    ]
}

/// Независимая f64-проверка Y'CbCr: клетка = центр + floor(координата + 0,5).
fn expected_ycbcr_cell(code: [u16; 3], gain: f64) -> (i64, i64) {
    let [r, g, b] = code.map(|channel| f64::from(channel) / 1023.0);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let cb = (b - y) / 1.8556;
    let cr = (r - y) / 1.5748;
    let cells_per_unit = 2.0 * 127.0 * gain;
    (
        (128.0 + (cb * cells_per_unit + 0.5).floor()) as i64,
        (128.0 + (-cr * cells_per_unit + 0.5).floor()) as i64,
    )
}

#[test]
fn every_gray_code_lands_exactly_in_the_centre_on_ycbcr_axes() {
    let rule = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    for code in 0..=1023u16 {
        let hits = hits_of_code(&rule, position(), [code; 3]);
        assert_eq!(hits.len(), 1, "код {code}");
        assert_eq!((hits[0].row, hits[0].column), (128, 128), "код {code}");
    }
}

#[test]
fn every_gray_code_lands_exactly_in_the_centre_on_oklab_axes_for_both_gamuts() {
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let rule = OklabVectorscopeRule::new(SIZE, 0.4, gamut).unwrap();
        for code in 0..=1023u16 {
            let hits = hits_of_code(&rule, position(), [code; 3]);
            assert_eq!(hits.len(), 1, "{gamut:?}, код {code}");
            assert_eq!((hits[0].row, hits[0].column), (128, 128), "{gamut:?}, код {code}");
        }
    }
}

#[test]
fn gray_picture_is_a_single_centre_point_on_both_axes() {
    let gray = picture(32, 8, |x, _| [code_of(f64::from(x) / 31.0); 3]);
    let ycbcr = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let oklab = OklabVectorscopeRule::new(SIZE, 0.4, DisplayGamut::DisplayP3).unwrap();
    for counters in [
        accumulate(&gray, SampleStride::EVERY_PIXEL, &ycbcr).unwrap(),
        accumulate(&gray, SampleStride::EVERY_PIXEL, &oklab).unwrap(),
    ] {
        assert_eq!(counters.count(0, 128, 128), Some(256));
        assert_eq!(counters.total(), 256);
    }
}

#[test]
fn bars_75_land_in_cells_computed_independently_in_f64() {
    for gain in [1.0_f32, 2.0] {
        let rule = YCbCrVectorscopeRule::new(SIZE, gain).unwrap();
        for code in bars_75() {
            let expected = expected_ycbcr_cell(code, f64::from(gain));
            let hits = hits_of_code(&rule, position(), code);
            let in_grid = (0..i64::from(SIZE)).contains(&expected.0) && (0..i64::from(SIZE)).contains(&expected.1);
            if in_grid {
                assert_eq!(hits.len(), 1, "{code:?} gain {gain}");
                assert_eq!(
                    (i64::from(hits[0].column), i64::from(hits[0].row)),
                    expected,
                    "{code:?} gain {gain}"
                );
            } else {
                assert!(
                    hits.is_empty(),
                    "{code:?} gain {gain}: вне сетки, попадания быть не должно"
                );
            }
        }
    }
}

#[test]
fn bars_75_have_the_expected_geometry() {
    let rule = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let cell = |code: [u16; 3]| {
        let hit = hits_of_code(&rule, position(), code)[0];
        (i64::from(hit.column) - 128, 128 - i64::from(hit.row))
    };
    let [red, green, blue, cyan, magenta, yellow] = bars_75().map(cell);
    // Красный: вправо-вверх-слабо, Cb<0 → влево? Cb красного отрицательна, Cr положительна.
    assert!(red.0 < 0 && red.1 > 0, "красный {red:?}");
    assert!(blue.0 > 0 && blue.1 < 0, "синий {blue:?}");
    assert!(green.0 < 0 && green.1 < 0, "зелёный {green:?}");
    // Дополнительные цвета — точно напротив основных (центральная симметрия, допуск 1 клетка на округление).
    for (primary, complement) in [(red, cyan), (green, magenta), (blue, yellow)] {
        assert!((primary.0 + complement.0).abs() <= 1 && (primary.1 + complement.1).abs() <= 1);
    }
}

#[test]
fn full_blue_and_red_stay_inside_the_grid_at_gain_one() {
    let rule = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    for code in [
        [1023, 0, 0],
        [0, 0, 1023],
        [0, 1023, 0],
        [1023, 1023, 0],
        [0, 1023, 1023],
        [1023, 0, 1023],
    ] {
        assert_eq!(hits_of_code(&rule, position(), code).len(), 1, "{code:?}");
    }
}

#[test]
fn gain_zooms_and_pushes_saturated_colors_out_of_the_grid() {
    let one = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let four = YCbCrVectorscopeRule::new(SIZE, 4.0).unwrap();
    let code = [code_of(0.6), code_of(0.5), code_of(0.5)];
    let near = hits_of_code(&one, position(), code)[0];
    let zoomed = hits_of_code(&four, position(), code)[0];
    // Красноватый цвет: Cr > 0 (вверх от центра), при усилении ×4 он вчетверо дальше от центра.
    assert!(zoomed.row < near.row);
    // Смещение растёт ×4 с точностью до округления клетки (floor у каждой из двух величин).
    let near_offset = 128 - i64::from(near.row);
    let zoomed_offset = 128 - i64::from(zoomed.row);
    assert!(
        (zoomed_offset - 4 * near_offset).abs() <= 4,
        "{near_offset} → {zoomed_offset}"
    );
    assert!(zoomed_offset > 3 * near_offset);
    // Полностью насыщенный красный при ×4 уходит за сетку: попадания нет.
    assert!(hits_of_code(&four, position(), [1023, 0, 0]).is_empty());
}

#[test]
fn oklab_primaries_land_where_f64_oklab_says() {
    for (gamut, primaries) in [
        (DisplayGamut::Srgb, Primaries::SRGB),
        (DisplayGamut::DisplayP3, Primaries::DISPLAY_P3),
    ] {
        let rule = OklabVectorscopeRule::new(SIZE, 0.4, gamut).unwrap();
        let to_xyz = primaries.rgb_to_xyz().unwrap();
        for code in bars_75().into_iter().chain([[1023, 0, 0], [0, 0, 1023], [0, 1023, 0]]) {
            let linear = code.map(|channel| {
                let encoded = f64::from(channel) / 1023.0;
                if encoded <= 0.04045 {
                    encoded / 12.92
                } else {
                    ((encoded + 0.055) / 1.055).powf(2.4)
                }
            });
            let lab = xyz_d65_to_oklab(to_xyz.apply(linear));
            let scale = 127.0 / 0.4;
            let expected_column = 128.0 + lab[1] * scale;
            let expected_row = 128.0 - lab[2] * scale;
            let hits = hits_of_code(&rule, position(), code);
            assert_eq!(hits.len(), 1, "{gamut:?} {code:?}");
            // Рабочая матрица OKLab нормирована по белому (a, b серого = 0): допуск — полторы клетки.
            assert!(
                (f64::from(hits[0].column) - expected_column).abs() <= 1.5,
                "{gamut:?} {code:?} столбец"
            );
            assert!(
                (f64::from(hits[0].row) - expected_row).abs() <= 1.5,
                "{gamut:?} {code:?} строка"
            );
        }
    }
}

#[test]
fn oklab_red_is_up_right_and_blue_is_down() {
    let rule = OklabVectorscopeRule::new(SIZE, 0.4, DisplayGamut::Srgb).unwrap();
    let red = hits_of_code(&rule, position(), [1023, 0, 0])[0];
    let blue = hits_of_code(&rule, position(), [0, 0, 1023])[0];
    assert!(red.column > 128 && red.row < 128, "красный {red:?}");
    assert!(blue.row > 128, "синий {blue:?}");
}

#[test]
fn both_axes_share_the_same_grid_shape() {
    let ycbcr = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let oklab = OklabVectorscopeRule::new(SIZE, 0.4, DisplayGamut::Srgb).unwrap();
    assert_eq!(
        scope_model::ScopeRule::shape(&ycbcr),
        scope_model::ScopeRule::shape(&oklab)
    );
}

#[test]
fn oklab_exposes_resolved_numbers_for_the_gpu_packer() {
    let rule = OklabVectorscopeRule::new(SIZE, 0.4, DisplayGamut::Srgb).unwrap();
    let table = rule.decode_table().values();
    assert_eq!(table[0], 0.0);
    assert_eq!(table[1023], 1.0);
    assert!(
        table.windows(2).all(|pair| pair[0] < pair[1]),
        "таблица обязана строго расти"
    );
    assert_eq!(rule.centre_offset(), 128.5);
    assert!((rule.scale() - 127.0 / 0.4).abs() < 1e-3);
    // Строки LMS у серого дают почти равные отклики: сумма строки ≈ 1.
    for row in rule.working_to_lms() {
        assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}

#[test]
fn flat_color_picture_counts_every_sample_once() {
    let rule = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let counters = accumulate(&flat(6, 5, [900, 300, 200]), SampleStride::EVERY_PIXEL, &rule).unwrap();
    assert_eq!(counters.total(), 30);
    assert_eq!(counters.counts().iter().filter(|&&count| count > 0).count(), 1);
}
