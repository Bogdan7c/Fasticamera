//! Экземпляр глифа или иконки (`GlyphInstance` в `ui_glyph.vert`, 5 × vec4 = 80 байт).

use ui_draw::atlas::AtlasRegion;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{IconItem, TextItem};
use ui_draw::geometry::PxRect;
use ui_draw::icons::icon;
use ui_draw::transform::Affine2;

use super::{put_affine_rows, put_floats, put_rect};

/// Размер экземпляра глифа в SSBO.
pub(crate) const GLYPH_INSTANCE_BYTES: usize = 80;

/// Один глиф: локальный прямоугольник, область атласа, преобразование, цвет с прозрачностью в альфе.
fn pack_glyph(
    rect: &PxRect,
    region: &AtlasRegion,
    transform: &Affine2,
    color: DisplayColor,
    opacity: f32,
    out: &mut Vec<u8>,
) {
    put_rect(out, rect);
    put_floats(
        out,
        &[region.uv_min[0], region.uv_min[1], region.uv_max[0], region.uv_max[1]],
    );
    put_affine_rows(out, transform);
    put_floats(out, &[color.r, color.g, color.b, color.a * opacity]);
}

/// Дописывает все глифы строки; возвращает их число.
pub(crate) fn pack_text(item: &TextItem, out: &mut Vec<u8>) -> usize {
    for glyph in &item.run.glyphs {
        pack_glyph(
            &glyph.quad,
            &glyph.region,
            &item.transform,
            item.color,
            item.opacity.get(),
            out,
        );
    }
    item.run.glyphs.len()
}

/// Дописывает иконку (один экземпляр). Ячейка атласа выступает за квадрат viewBox на поле расстояний:
/// `quad_in_square` в долях квадрата, квадрат лежит в рамке `size_px` с центром в нуле.
pub(crate) fn pack_icon(item: &IconItem, out: &mut Vec<u8>) {
    let glyph = icon(item.icon);
    let (width, height) = (item.size_px.width, item.size_px.height);
    let quad = glyph.quad_in_square;
    let rect = PxRect::new(
        ui_draw::geometry::PxPoint::new(
            -width * 0.5 + quad.origin.x * width,
            -height * 0.5 + quad.origin.y * height,
        ),
        ui_draw::geometry::PxSize::new(quad.size.width * width, quad.size.height * height),
    );
    pack_glyph(
        &rect,
        &glyph.region,
        &item.transform,
        item.color,
        item.opacity.get(),
        out,
    );
}
