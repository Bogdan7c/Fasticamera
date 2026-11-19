//! Запечённый атлас: параметры, покрытие манифеста и побайтное совпадение с повторным запеканием.

// Это тест: вспомогательные функции падают через expect/panic, как и сами проверки.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use support::font_bake::bake_face;
use support::glyph_cell::render_glyph_cell;
use ui_draw::atlas::sdf_atlas;
use ui_draw::text::{Figures, FontFace, TextStyle, layout_line};

fn style(face: FontFace, figures: Figures) -> TextStyle {
    TextStyle {
        face,
        size_px: 64.0,
        tracking_em: 0.0,
        figures,
    }
}

#[test]
fn atlas_parameters_match_the_manifest_and_the_texel_buffer_is_complete() {
    let manifest = support::load_manifest();
    let atlas = sdf_atlas();
    assert_eq!(atlas.width, manifest.atlas.width_texels);
    assert_eq!(atlas.texels.len(), (atlas.width * atlas.height) as usize);
    assert_eq!(atlas.spread_texels, manifest.atlas.spread_texels as f32);
    assert_eq!(atlas.em_texels, manifest.atlas.em_texels as f32);
    // В атласе есть и «внутри» (<128), и «снаружи» (>128): он не пустой и не залит одним значением.
    assert!(atlas.texels.iter().any(|&byte| byte < 100));
    assert!(atlas.texels.iter().any(|&byte| byte > 200));
}

#[test]
fn every_manifest_character_has_a_glyph_in_every_face_and_figure_set() {
    let manifest = support::load_manifest();
    for face in FontFace::ALL {
        for figures in [Figures::Proportional, Figures::Tabular] {
            for character in manifest.characters().expect("набор символов") {
                let run = layout_line(&character.to_string(), &style(face, figures))
                    .unwrap_or_else(|error| panic!("{face:?} {character:?}: {error}"));
                if character == ' ' {
                    assert!(
                        run.glyphs.is_empty() && run.advance_px > 0.0,
                        "пробел — только продвижение"
                    );
                } else {
                    assert_eq!(run.glyphs.len(), 1, "{face:?} {character:?}: ровно один глиф");
                }
            }
        }
    }
}

#[test]
fn baked_cells_equal_a_fresh_run_of_the_generator_on_the_same_inputs() {
    let manifest = support::load_manifest();
    let font_bytes = support::load_font_bytes(&manifest);
    let characters = manifest.characters().expect("набор символов");
    let atlas = sdf_atlas();
    let (atlas_width, atlas_height) = (atlas.width as f32, atlas.height as f32);

    let mut compared_cells = BTreeSet::new();
    for (face_index, face) in FontFace::ALL.into_iter().enumerate() {
        let baked = bake_face(
            &font_bytes,
            &manifest.faces[face_index],
            &characters,
            &manifest.disabled_features,
        )
        .expect("повторное снятие шрифта");
        for (figures, glyphs) in [
            (Figures::Proportional, &baked.proportional),
            (Figures::Tabular, &baked.tabular),
        ] {
            for glyph in glyphs.iter().filter(|glyph| glyph.character != ' ') {
                let fresh = render_glyph_cell(
                    &baked.shapes[&glyph.glyph_id],
                    baked.metrics.units_per_em,
                    manifest.atlas.em_texels,
                    manifest.atlas.spread_texels,
                )
                .expect("у видимого глифа есть контур");

                // Где этот глиф лежит в атласе, узнаём из публичной вёрстки.
                let run = layout_line(&glyph.character.to_string(), &style(face, figures)).expect("в наборе");
                let placed = &run.glyphs[0];
                let left = (placed.region.uv_min[0] * atlas_width).round() as usize;
                let top = (placed.region.uv_min[1] * atlas_height).round() as usize;
                let width = ((placed.region.uv_max[0] - placed.region.uv_min[0]) * atlas_width).round() as u32;
                let height = ((placed.region.uv_max[1] - placed.region.uv_min[1]) * atlas_height).round() as u32;
                assert_eq!(
                    (width, height),
                    (fresh.width_texels, fresh.height_texels),
                    "{face:?} {:?}: размер ячейки",
                    glyph.character
                );

                // Рамка на экране: при кегле = em_texels один тексель — один пиксель.
                let expected_x = fresh.left_em as f32 * 64.0;
                let expected_y = fresh.top_em as f32 * 64.0;
                assert!(
                    (placed.quad.origin.x - expected_x).abs() < 1e-3
                        && (placed.quad.origin.y - expected_y).abs() < 1e-3
                );
                assert!((placed.quad.size.width - width as f32).abs() < 1e-3);

                for row in 0..height as usize {
                    let atlas_row = &atlas.texels[(top + row) * atlas.width as usize + left..][..width as usize];
                    let fresh_row = &fresh.texels[row * width as usize..][..width as usize];
                    assert_eq!(
                        atlas_row, fresh_row,
                        "{face:?} {:?}: строка {row} атласа отличается от повторного запекания",
                        glyph.character
                    );
                }
                compared_cells.insert((face_index, glyph.glyph_id));
            }
        }
    }
    // Сверено не меньше, чем есть видимых символов (цифры tnum добавляют ячейки сверх этого).
    assert!(
        compared_cells.len() >= 2 * (characters.len() - 1),
        "сверено {} ячеек",
        compared_cells.len()
    );
}

#[test]
fn letter_o_is_a_ring_in_the_atlas_not_a_filled_disc() {
    let atlas = sdf_atlas();
    let run = layout_line("O", &style(FontFace::Regular, Figures::Proportional)).expect("в наборе");
    let region = run.glyphs[0].region;
    let left = (region.uv_min[0] * atlas.width as f32).round() as usize;
    let top = (region.uv_min[1] * atlas.height as f32).round() as usize;
    let width = ((region.uv_max[0] - region.uv_min[0]) * atlas.width as f32).round() as usize;
    let height = ((region.uv_max[1] - region.uv_min[1]) * atlas.height as f32).round() as usize;
    let texel = |column: usize, row: usize| atlas.texels[(top + row) * atlas.width as usize + left + column];
    // Центр «O» — в дырке (снаружи формы, > 128); середина левой стенки — в штрихе (< 128).
    assert!(texel(width / 2, height / 2) > 128, "центр «O» должен быть в дырке");
    let stem = (0..width / 2)
        .map(|column| texel(column, height / 2))
        .min()
        .expect("строка не пуста");
    assert!(stem < 128, "левая стенка «O» должна быть внутри формы, минимум {stem}");
    assert_eq!(texel(0, 0), 255, "угол ячейки далеко снаружи");
}

#[test]
fn glyph_quad_places_the_letter_edges_where_the_font_outline_says() {
    let manifest = support::load_manifest();
    let bytes = support::load_font_bytes(&manifest);
    let font = skrifa::FontRef::new(&bytes).expect("skrifa открывает шрифт");
    let spec = &manifest.faces[0];
    let location =
        skrifa::MetadataProvider::axes(&font).location(spec.axes.iter().map(|(tag, value)| (tag.as_str(), *value)));
    let glyph_id = skrifa::MetadataProvider::charmap(&font)
        .map('H')
        .expect("H есть в шрифте");
    let bounds = skrifa::MetadataProvider::glyph_metrics(&font, skrifa::instance::Size::unscaled(), &location)
        .bounds(glyph_id)
        .expect("у H есть рамка");
    let units_per_em =
        skrifa::MetadataProvider::metrics(&font, skrifa::instance::Size::unscaled(), &location).units_per_em;

    // Кегль равен em атласа, поэтому один тексель — один пиксель.
    let size_px = manifest.atlas.em_texels as f32;
    let run = layout_line(
        "H",
        &TextStyle {
            face: FontFace::Regular,
            size_px,
            tracking_em: 0.0,
            figures: Figures::Proportional,
        },
    )
    .expect("в наборе");
    let placed = &run.glyphs[0];
    let atlas = sdf_atlas();
    let left = (placed.region.uv_min[0] * atlas.width as f32).round() as usize;
    let top = (placed.region.uv_min[1] * atlas.height as f32).round() as usize;
    let width = ((placed.region.uv_max[0] - placed.region.uv_min[0]) * atlas.width as f32).round() as usize;
    let height = ((placed.region.uv_max[1] - placed.region.uv_min[1]) * atlas.height as f32).round() as usize;
    let texel = |column: usize, row: usize| atlas.texels[(top + row) * atlas.width as usize + left + column];

    // Левый край левой стойки: первый «внутренний» тексель средней строки у верха стойки (выше перекладины).
    let probe_row = height / 4;
    let first_inside_column = (0..width)
        .find(|&column| texel(column, probe_row) < 128)
        .expect("стойка H");
    let measured_left = placed.quad.origin.x + first_inside_column as f32;
    let expected_left = bounds.x_min / f32::from(units_per_em) * size_px;
    assert!(
        (measured_left - expected_left).abs() <= 1.5,
        "левый край: {measured_left} против {expected_left}"
    );

    // Верх стойки: первый внутренний тексель сверху в колонке стойки — это высота заглавной H над базовой линией.
    let stem_column = first_inside_column + 3;
    let first_inside_row = (0..height)
        .find(|&row| texel(stem_column, row) < 128)
        .expect("верх стойки");
    let measured_top = placed.quad.origin.y + first_inside_row as f32;
    let expected_top = -bounds.y_max / f32::from(units_per_em) * size_px;
    assert!(
        (measured_top - expected_top).abs() <= 1.5,
        "верх: {measured_top} против {expected_top}"
    );
    // Низ стойки лежит на базовой линии (y = 0).
    let last_inside_row = (0..height)
        .rev()
        .find(|&row| texel(stem_column, row) < 128)
        .expect("низ стойки");
    let measured_bottom = placed.quad.origin.y + last_inside_row as f32 + 1.0;
    assert!(
        measured_bottom.abs() <= 1.5,
        "низ H должен стоять на базовой линии: {measured_bottom}"
    );
}
