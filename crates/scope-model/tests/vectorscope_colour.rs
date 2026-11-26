//! Цвет вектороскопа «по месту» (решение 267): цвет клетки = цвет того места, куда правило кладёт отсчёт.
//! Проверяем через настоящие попадания правил: код цвета ложится в клетку, а цвет клетки обязан быть цветом
//! этого кода по оттенку, на обеих осях и обоих охватах экрана.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::code_of;
use isp_model::image::DisplayGamut;
use scope_model::oklab_rule::OklabVectorscopeRule;
use scope_model::vectorscope_ycbcr::YCbCrVectorscopeRule;
use scope_model::{PositionPalette, SamplePosition, ScopeRule, hits_of_code};

const SIZE: u32 = 256;
const LIGHTNESS: f32 = 0.70;

fn position() -> SamplePosition {
    SamplePosition { x: 0, picture_width: 1 }
}

/// Цвета полос 75 %: (название, код, каналы, которые должны быть «включёнными» в оттенке).
fn bar_colours() -> [(&'static str, [u16; 3], [bool; 3]); 6] {
    let on = code_of(0.75);
    [
        ("красный", [on, 0, 0], [true, false, false]),
        ("зелёный", [0, on, 0], [false, true, false]),
        ("синий", [0, 0, on], [false, false, true]),
        ("голубой", [0, on, on], [false, true, true]),
        ("пурпурный", [on, 0, on], [true, false, true]),
        ("жёлтый", [on, on, 0], [true, true, false]),
    ]
}

/// Все палитры: две оси, у OKLab оба охвата.
fn palettes() -> Vec<(&'static str, Box<dyn ScopeRule>, PositionPalette)> {
    let ycbcr = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap();
    let ycbcr_palette = ycbcr.position_palette(LIGHTNESS);
    let mut all: Vec<(&'static str, Box<dyn ScopeRule>, PositionPalette)> =
        vec![("Y'CbCr", Box::new(ycbcr), ycbcr_palette)];
    for (name, gamut) in [
        ("OKLab sRGB", DisplayGamut::Srgb),
        ("OKLab P3", DisplayGamut::DisplayP3),
    ] {
        let rule = OklabVectorscopeRule::new(SIZE, 0.4, gamut).unwrap();
        let palette = rule.position_palette(LIGHTNESS);
        all.push((name, Box::new(rule), palette));
    }
    all
}

#[test]
fn the_colour_of_a_cell_has_the_hue_of_the_code_that_lands_there() {
    for (axes, rule, palette) in palettes() {
        for (name, code, active) in bar_colours() {
            let hit = hits_of_code(rule.as_ref(), position(), code)[0];
            let colour = palette.colour_at(hit.column, hit.row);
            // Включённые каналы оттенка заметно выше выключенных (светлота одна, различие — в хроме).
            let on_min = (0..3)
                .filter(|&c| active[c])
                .map(|c| colour[c])
                .fold(f32::INFINITY, f32::min);
            let off_max = (0..3)
                .filter(|&c| !active[c])
                .map(|c| colour[c])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                on_min > off_max + 0.05,
                "{axes}, {name}: клетка ({}, {}) окрашена {colour:?}, оттенок не тот",
                hit.column,
                hit.row
            );
        }
    }
}

#[test]
fn every_cell_of_the_grid_has_a_finite_colour_inside_the_screen_range_and_the_centre_is_neutral() {
    for (axes, rule, palette) in palettes() {
        for row in 0..SIZE {
            for column in 0..SIZE {
                let colour = palette.colour_at(column, row);
                assert!(
                    colour
                        .iter()
                        .all(|channel| channel.is_finite() && (-1e-6..=1.0 + 1e-6).contains(channel)),
                    "{axes}: клетка ({column}, {row}) = {colour:?}"
                );
            }
        }
        // Серый всегда в центральной клетке: она красится нейтралью (ветка r = 0 до вычисления угла).
        let gray_hit = hits_of_code(rule.as_ref(), position(), [512; 3])[0];
        assert_eq!((gray_hit.column, gray_hit.row), (SIZE / 2, SIZE / 2), "{axes}");
        assert_eq!(
            palette.colour_at(gray_hit.column, gray_hit.row),
            palette.neutral(),
            "{axes}"
        );
    }
}

#[test]
fn the_neutral_is_the_configured_lightness_on_ycbcr_and_a_gray_of_that_lightness_on_oklab() {
    let ycbcr = YCbCrVectorscopeRule::new(SIZE, 1.0)
        .unwrap()
        .position_palette(LIGHTNESS);
    assert_eq!(ycbcr.neutral(), [LIGHTNESS; 3]);
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let neutral = OklabVectorscopeRule::new(SIZE, 0.4, gamut)
            .unwrap()
            .position_palette(LIGHTNESS)
            .neutral();
        assert!(
            (neutral[0] - neutral[1]).abs() < 1e-3 && (neutral[1] - neutral[2]).abs() < 1e-3,
            "{neutral:?}"
        );
        // OKLab L = 0,70 → линейный свет 0,343 → кодировка sRGB ≈ 0,62 (светлее/темнее заданной не должно уходить далеко).
        assert!((neutral[0] - 0.62).abs() < 0.02, "{gamut:?}: {neutral:?}");
    }
    // Другая светлота — другая нейтраль и другие края (таблица действительно зависит от настройки).
    let brighter = YCbCrVectorscopeRule::new(SIZE, 1.0).unwrap().position_palette(0.8);
    assert_eq!(brighter.neutral(), [0.8; 3]);
    assert_ne!(
        brighter.entries(),
        YCbCrVectorscopeRule::new(SIZE, 1.0)
            .unwrap()
            .position_palette(LIGHTNESS)
            .entries()
    );
}

#[test]
fn colours_saturate_toward_the_edge_and_stop_at_the_screen_gamut_limit() {
    let palette = YCbCrVectorscopeRule::new(SIZE, 1.0)
        .unwrap()
        .position_palette(LIGHTNESS);
    let centre = SIZE / 2;
    // Вправо от центра (Cb > 0, синий): чем дальше, тем синее; за пределом охвата цвет не меняется.
    let blue_of = |offset: u32| palette.colour_at(centre + offset, centre)[2];
    assert!(blue_of(5) > blue_of(1));
    assert!(blue_of(30) > blue_of(5));
    assert_eq!(
        palette.colour_at(centre + 126, centre),
        palette.colour_at(centre + 120, centre)
    );
    // Никакой канал не вышел за экран даже на краю сетки.
    assert!(
        palette
            .colour_at(centre + 126, centre)
            .iter()
            .all(|channel| (0.0..=1.0).contains(channel))
    );
}
