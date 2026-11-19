//! Запекание целиком: манифест + шрифт + SVG → байты атласа и исходный код таблиц.
//!
//! Порядок детерминирован: начертания — как в манифесте, глифы — по номеру, иконки — как в манифесте,
//! без `HashMap`. Один и тот же вход даёт побайтно один и тот же выход.

use super::codegen;
use super::error::{BakeError, BakeResult};
use super::font_bake::{FaceBake, bake_face};
use super::glyph_cell::{GlyphCell, render_glyph_cell};
use super::manifest::BakeManifest;
use super::pack::{CellSize, pack_shelves};
use super::svg::{IconCell, parse_svg, render_icon_cell};
use std::collections::BTreeMap;
use std::path::Path;

/// Значение пустого места атласа: «далеко снаружи».
pub const EMPTY_TEXEL: u8 = 255;

/// Результат запекания.
#[derive(Debug, Clone)]
pub struct BakeOutput {
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// Одноканальный SDF-атлас построчно.
    pub texels: Vec<u8>,
    /// Таблицы шрифта (включается в `text/tables.rs`).
    pub font_tables_source: String,
    /// Таблицы иконок (включается в `icons.rs`).
    pub icon_tables_source: String,
    /// Константы атласа (включается в `atlas.rs`).
    pub atlas_meta_source: String,
}

/// Место глифа в атласе: начертание + номер глифа (у цифр `tnum` номер свой).
pub type GlyphKey = (usize, u32);

/// Ячейки, готовые к упаковке, в детерминированном порядке.
pub struct RenderedCells {
    pub glyphs: Vec<(GlyphKey, GlyphCell)>,
    pub icons: Vec<IconCell>,
}

/// Запекает всё по манифесту; пути в манифесте — относительно `assets_dir`.
pub fn bake_all(manifest: &BakeManifest, assets_dir: &Path) -> BakeResult<BakeOutput> {
    let font_path = assets_dir.join(&manifest.font);
    let font_bytes =
        std::fs::read(&font_path).map_err(|error| BakeError::new(format!("{}: {error}", font_path.display())))?;
    let characters = manifest.characters()?;
    let faces = bake_faces(manifest, &font_bytes, &characters)?;
    let cells = render_cells(manifest, assets_dir, &faces)?;

    let sizes: Vec<CellSize> = cells
        .glyphs
        .iter()
        .map(|(_, cell)| CellSize {
            width: cell.width_texels,
            height: cell.height_texels,
        })
        .chain(cells.icons.iter().map(|cell| CellSize {
            width: cell.size_texels,
            height: cell.size_texels,
        }))
        .collect();
    let packing = pack_shelves(&sizes, manifest.atlas.width_texels, manifest.atlas.gutter_texels)?;

    let mut texels = vec![EMPTY_TEXEL; (manifest.atlas.width_texels * packing.atlas_height) as usize];
    let mut glyph_places = BTreeMap::new();
    for (index, (key, cell)) in cells.glyphs.iter().enumerate() {
        let place = packing.placements[index];
        blit(
            &mut texels,
            manifest.atlas.width_texels,
            place.x,
            place.y,
            cell.width_texels,
            &cell.texels,
        );
        glyph_places.insert(*key, (place, cell));
    }
    let mut icon_places = Vec::new();
    for (offset, cell) in cells.icons.iter().enumerate() {
        let place = packing.placements[cells.glyphs.len() + offset];
        blit(
            &mut texels,
            manifest.atlas.width_texels,
            place.x,
            place.y,
            cell.size_texels,
            &cell.texels,
        );
        icon_places.push((place, cell.size_texels));
    }

    Ok(BakeOutput {
        atlas_width: manifest.atlas.width_texels,
        atlas_height: packing.atlas_height,
        texels,
        font_tables_source: codegen::font_tables(&faces, &glyph_places),
        icon_tables_source: codegen::icon_tables(manifest, &icon_places),
        atlas_meta_source: codegen::atlas_meta(manifest, packing.atlas_height),
    })
}

/// Снимает данные всех начертаний (параллельно: начертания независимы, порядок результата сохраняется).
pub fn bake_faces(manifest: &BakeManifest, font_bytes: &[u8], characters: &[char]) -> BakeResult<Vec<FaceBake>> {
    std::thread::scope(|scope| {
        let workers: Vec<_> = manifest
            .faces
            .iter()
            .map(|spec| scope.spawn(|| bake_face(font_bytes, spec, characters, &manifest.disabled_features)))
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| BakeError::new("поток запекания начертания аварийно завершился"))?
            })
            .collect()
    })
}

/// Рисует SDF-ячейки глифов и иконок.
pub fn render_cells(manifest: &BakeManifest, assets_dir: &Path, faces: &[FaceBake]) -> BakeResult<RenderedCells> {
    let atlas = &manifest.atlas;
    let mut glyphs = Vec::new();
    for (face_index, face) in faces.iter().enumerate() {
        for (&glyph_id, shape) in &face.shapes {
            let cell = render_glyph_cell(shape, face.metrics.units_per_em, atlas.em_texels, atlas.spread_texels)
                .ok_or_else(|| BakeError::new(format!("начертание {}: у глифа {glyph_id} пустой контур", face.name)))?;
            glyphs.push(((face_index, glyph_id), cell));
        }
    }
    let mut icons = Vec::new();
    for icon in &manifest.icons {
        let path = assets_dir.join(&icon.file);
        let text =
            std::fs::read_to_string(&path).map_err(|error| BakeError::new(format!("{}: {error}", path.display())))?;
        let parsed = parse_svg(&icon.file, &text)?;
        icons.push(render_icon_cell(
            &icon.file,
            &parsed,
            icon.kind,
            icon.stroke_width,
            atlas.icon_texels,
            atlas.spread_texels,
        )?);
    }
    Ok(RenderedCells { glyphs, icons })
}

/// Копирует ячейку в атлас построчно.
fn blit(atlas: &mut [u8], atlas_width: u32, x: u32, y: u32, cell_width: u32, cell: &[u8]) {
    let cell_width = cell_width as usize;
    for (row, line) in cell.chunks_exact(cell_width).enumerate() {
        let start = (y as usize + row) * atlas_width as usize + x as usize;
        atlas[start..start + cell_width].copy_from_slice(line);
    }
}
