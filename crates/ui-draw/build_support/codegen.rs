//! Генерация исходного кода таблиц: рантайм получает только числа, без шрифтовых крейтов.

use super::bake::GlyphKey;
use super::font_bake::{BakedGlyph, FaceBake, KernPair};
use super::glyph_cell::GlyphCell;
use super::manifest::{BakeManifest, upper_camel_case};
use super::pack::CellPlacement;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Места ячеек глифов: ключ → (положение в атласе, ячейка).
pub type GlyphPlaces<'a> = BTreeMap<GlyphKey, (CellPlacement, &'a GlyphCell)>;

/// Литерал `char` в виде `'\u{41}'` — безопасен для любых символов.
fn char_literal(character: char) -> String {
    format!("'\\u{{{:X}}}'", character as u32)
}

/// Литерал `f32`, который читается обратно в то же число.
fn f32_literal(value: f64) -> String {
    format!("{:?}", value as f32)
}

/// Таблицы шрифта: enum начертаний и данные всех начертаний.
pub fn font_tables(faces: &[FaceBake], places: &GlyphPlaces<'_>) -> String {
    let mut source =
        String::from("// Сгенерировано build.rs (ui-draw/build_support/codegen.rs). Не править руками.\n\n");
    let variants: Vec<String> = faces.iter().map(|face| upper_camel_case(&face.name)).collect();
    let _ = writeln!(
        source,
        "/// Начертания шрифта, запечённые в атлас (манифест `assets/bake.json`)."
    );
    let _ = writeln!(
        source,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum FontFace {{"
    );
    for variant in &variants {
        let _ = writeln!(source, "    {variant},");
    }
    let _ = writeln!(source, "}}\n");
    let _ = writeln!(
        source,
        "impl FontFace {{\n    /// Все начертания в порядке манифеста.\n    pub const ALL: [FontFace; {}] = [",
        variants.len()
    );
    for variant in &variants {
        let _ = writeln!(source, "        FontFace::{variant},");
    }
    let _ = writeln!(source, "    ];\n}}\n");

    for (face_index, face) in faces.iter().enumerate() {
        let prefix = face.name.to_ascii_uppercase();
        glyph_array(
            &mut source,
            &format!("{prefix}_PROPORTIONAL_GLYPHS"),
            face_index,
            &face.proportional,
            places,
        );
        glyph_array(
            &mut source,
            &format!("{prefix}_TABULAR_GLYPHS"),
            face_index,
            &face.tabular,
            places,
        );
        kern_array(
            &mut source,
            &format!("{prefix}_PROPORTIONAL_KERNING"),
            &face.kerning_proportional,
        );
        kern_array(&mut source, &format!("{prefix}_TABULAR_KERNING"), &face.kerning_tabular);
    }
    let _ = writeln!(
        source,
        "/// Таблицы начертаний в порядке `FontFace::ALL`.\npub(super) static FACE_TABLES: [FaceTables; {}] = [",
        faces.len()
    );
    for face in faces {
        let prefix = face.name.to_ascii_uppercase();
        let _ = writeln!(
            source,
            "    FaceTables {{\n        units_per_em: {},\n        cap_height_units: {},\n        ascent_units: {},\n        descent_units: {},\n        proportional: FigureTable {{ glyphs: &{prefix}_PROPORTIONAL_GLYPHS, kerning: &{prefix}_PROPORTIONAL_KERNING }},\n        tabular: FigureTable {{ glyphs: &{prefix}_TABULAR_GLYPHS, kerning: &{prefix}_TABULAR_KERNING }},\n    }},",
            face.metrics.units_per_em,
            face.metrics.cap_height_units,
            face.metrics.ascent_units,
            face.metrics.descent_units,
        );
    }
    let _ = writeln!(source, "];");
    source
}

fn glyph_array(source: &mut String, name: &str, face_index: usize, glyphs: &[BakedGlyph], places: &GlyphPlaces<'_>) {
    let _ = writeln!(source, "static {name}: [GlyphEntry; {}] = [", glyphs.len());
    for glyph in glyphs {
        let cell = match places.get(&(face_index, glyph.glyph_id)) {
            Some((place, cell)) => format!(
                "Some(GlyphCellEntry {{ atlas_x: {}, atlas_y: {}, width_texels: {}, height_texels: {}, left_em: {}, top_em: {} }})",
                place.x,
                place.y,
                cell.width_texels,
                cell.height_texels,
                f32_literal(cell.left_em),
                f32_literal(cell.top_em)
            ),
            None => "None".to_string(),
        };
        let _ = writeln!(
            source,
            "    GlyphEntry {{ character: {}, advance_units: {}, cell: {cell} }},",
            char_literal(glyph.character),
            glyph.advance_units
        );
    }
    let _ = writeln!(source, "];\n");
}

fn kern_array(source: &mut String, name: &str, pairs: &[KernPair]) {
    let _ = writeln!(source, "static {name}: [KernEntry; {}] = [", pairs.len());
    for pair in pairs {
        let _ = writeln!(
            source,
            "    KernEntry {{ first: {}, second: {}, value_units: {} }},",
            char_literal(pair.first),
            char_literal(pair.second),
            pair.value_units
        );
    }
    let _ = writeln!(source, "];\n");
}

/// Таблицы иконок: enum `IconId` и места ячеек.
pub fn icon_tables(manifest: &BakeManifest, places: &[(CellPlacement, u32)]) -> String {
    let mut source =
        String::from("// Сгенерировано build.rs (ui-draw/build_support/codegen.rs). Не править руками.\n\n");
    let variants: Vec<String> = manifest.icons.iter().map(|icon| upper_camel_case(&icon.id)).collect();
    let _ = writeln!(source, "/// Иконки, запечённые в атлас (манифест `assets/bake.json`).");
    let _ = writeln!(
        source,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\npub enum IconId {{"
    );
    for variant in &variants {
        let _ = writeln!(source, "    {variant},");
    }
    let _ = writeln!(source, "}}\n");
    let _ = writeln!(
        source,
        "impl IconId {{\n    /// Все иконки в порядке манифеста.\n    pub const ALL: [IconId; {}] = [",
        variants.len()
    );
    for variant in &variants {
        let _ = writeln!(source, "        IconId::{variant},");
    }
    let _ = writeln!(source, "    ];\n}}\n");
    let _ = writeln!(
        source,
        "/// Места ячеек в порядке `IconId::ALL`.\nstatic ICON_CELLS: [IconCellEntry; {}] = [",
        places.len()
    );
    for (place, size) in places {
        let _ = writeln!(
            source,
            "    IconCellEntry {{ atlas_x: {}, atlas_y: {}, size_texels: {size} }},",
            place.x, place.y
        );
    }
    let _ = writeln!(source, "];");
    source
}

/// Константы атласа и байты текстуры.
pub fn atlas_meta(manifest: &BakeManifest, atlas_height: u32) -> String {
    let atlas = &manifest.atlas;
    format!(
        "// Сгенерировано build.rs (ui-draw/build_support/codegen.rs). Не править руками.\n\n\
         pub(super) const WIDTH_TEXELS: u32 = {};\n\
         pub(super) const HEIGHT_TEXELS: u32 = {atlas_height};\n\
         pub(super) const SPREAD_TEXELS: u32 = {};\n\
         pub(super) const EM_TEXELS: u32 = {};\n\
         pub(super) const ICON_TEXELS: u32 = {};\n\
         pub(super) static TEXELS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \"/sdf_atlas.bin\"));\n",
        atlas.width_texels, atlas.spread_texels, atlas.em_texels, atlas.icon_texels
    )
}
