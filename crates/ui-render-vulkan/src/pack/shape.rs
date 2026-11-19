//! Экземпляр закрашенной фигуры (`ShapeInstance` в `ui_shape_data.glsl`, 8 × vec4 = 128 байт).

use ui_draw::draw_list::{DrawListError, ShapeItem};
use ui_draw::geometry::PxRect;
use ui_draw::shape::ShapeGeometry;

use super::{put_affine_rows, put_floats, put_rect};
use crate::error::UiRenderError;

/// Размер экземпляра фигуры в SSBO.
pub(crate) const SHAPE_INSTANCE_BYTES: usize = 128;

/// Параметры SDF фигуры для шейдера: половина ширины, половина высоты, радиус, показатель.
/// Источник правды — `ShapeGeometry::sdf_parameters` в ui-draw (его же использует эталон формулы).
pub(crate) fn geometry_parameters(geometry: &ShapeGeometry) -> [f32; 4] {
    let sdf = geometry.sdf_parameters();
    [sdf.half_size.width, sdf.half_size.height, sdf.radius, sdf.exponent]
}

/// Дописывает экземпляр фигуры. `item_index` — номер элемента списка (для ошибки вырожденной матрицы).
pub(crate) fn pack_shape(
    item_index: usize,
    item: &ShapeItem,
    bounds: &PxRect,
    out: &mut Vec<u8>,
) -> Result<(), UiRenderError> {
    // Обратную матрицу считает CPU (как эталон), шейдер её не обращает.
    let inverse = item
        .transform
        .inverse()
        .map_err(|source| DrawListError::DegenerateTransform {
            item: item_index,
            source,
        })?;
    let start = out.len();
    put_rect(out, bounds);
    put_affine_rows(out, &inverse);
    put_floats(out, &geometry_parameters(&item.geometry));
    put_floats(out, &[item.fill.r, item.fill.g, item.fill.b, item.fill.a]);
    let stroke_color = item
        .stroke
        .map_or([0.0; 4], |s| [s.color.r, s.color.g, s.color.b, s.color.a]);
    put_floats(out, &stroke_color);
    let stroke_width = item.stroke.map_or(0.0, |s| s.width_px);
    put_floats(out, &[stroke_width, item.opacity.get(), 0.0, 0.0]);
    let shadow = item
        .shadow
        .map_or([0.0; 4], |s| [s.radius_px, s.alpha, s.offset_px, 0.0]);
    put_floats(out, &shadow);
    debug_assert_eq!(out.len() - start, SHAPE_INSTANCE_BYTES);
    Ok(())
}
