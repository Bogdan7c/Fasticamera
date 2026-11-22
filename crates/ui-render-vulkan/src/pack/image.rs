//! Экземпляр картинки (`ImageInstance` в `ui_image_data.glsl`, 5 × vec4 = 80 байт).

use ui_draw::draw_list::{DrawListError, ImageItem, clip_geometry, cover_uv_scale};
use ui_draw::geometry::PxRect;

use super::{put_affine_rows, put_floats, put_rect};
use crate::error::UiRenderError;

/// Размер экземпляра картинки в SSBO.
pub(crate) const IMAGE_INSTANCE_BYTES: usize = 80;

/// Дописывает экземпляр картинки.
///
/// Раскладка: `bounds`, две строки ОБРАТНОЙ матрицы, `geom` (параметры SDF формы обрезки — из той же
/// геометрии, что у эталона), `uv` = (единиц `uv` на локальный пиксель по x и по y, прозрачность элемента,
/// номер текстуры). Номер лежит в `f32` (целые до 2²⁴ точны, у нас их единицы) — так весь экземпляр
/// состоит из одних `vec4`.
///
/// `item_index` — номер элемента списка (для ошибки вырожденной матрицы), `texture_index` — номер сэмплера кадра.
pub(crate) fn pack_image(
    item_index: usize,
    item: &ImageItem,
    bounds: &PxRect,
    texture_index: usize,
    out: &mut Vec<u8>,
) -> Result<(), UiRenderError> {
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
    let sdf = clip_geometry(item.clip, item.size_px).sdf_parameters();
    put_floats(
        out,
        &[sdf.half_size.width, sdf.half_size.height, sdf.radius, sdf.exponent],
    );
    let uv_scale = cover_uv_scale(item.fit, item.size_px, item.picture.size());
    put_floats(
        out,
        &[uv_scale[0], uv_scale[1], item.opacity.get(), texture_index as f32],
    );
    debug_assert_eq!(out.len() - start, IMAGE_INSTANCE_BYTES);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "тесты")]

    use super::*;
    use ui_draw::color::Opacity;
    use ui_draw::draw_list::{DrawItem, ImageClip, ImageFit};
    use ui_draw::geometry::PxSize;
    use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};
    use ui_draw::transform::Affine2;

    fn item(transform: Affine2) -> ImageItem {
        let picture =
            UiPicture::from_rgba8(PictureSize::new(40, 20), PictureEncoding::Srgb, vec![0; 40 * 20 * 4]).unwrap();
        ImageItem {
            picture,
            transform,
            size_px: PxSize::new(20.0, 20.0),
            fit: ImageFit::Cover,
            clip: ImageClip::Circle,
            opacity: Opacity::new(0.25).unwrap(),
        }
    }

    fn float_at(bytes: &[u8], index: usize) -> f32 {
        f32::from_ne_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
    }

    #[test]
    fn instance_carries_geometry_cover_scale_opacity_and_texture_index() {
        let item = item(Affine2::translate(50.0, 60.0));
        let bounds = DrawItem::Image(item.clone()).bounds_px().unwrap();
        let mut bytes = Vec::new();
        pack_image(3, &item, &bounds, 1, &mut bytes).unwrap();
        assert_eq!(bytes.len(), IMAGE_INSTANCE_BYTES);
        // Обратный перенос (−50, −60) в строках матрицы: tx, ty.
        assert_eq!((float_at(&bytes, 6), float_at(&bytes, 10)), (-50.0, -60.0));
        // Круг диаметром 20: половина стороны 10, радиус 10.
        assert_eq!(
            (float_at(&bytes, 12), float_at(&bytes, 13), float_at(&bytes, 14)),
            (10.0, 10.0, 10.0)
        );
        // Cover рамки 20×20 над источником 40×20: по x 1/40, по y 1/20.
        assert_eq!((float_at(&bytes, 16), float_at(&bytes, 17)), (1.0 / 40.0, 1.0 / 20.0));
        assert_eq!(
            (float_at(&bytes, 18), float_at(&bytes, 19)),
            (0.25, 1.0),
            "прозрачность и номер текстуры"
        );
    }

    #[test]
    fn degenerate_transform_is_reported_with_the_item_number() {
        let item = item(Affine2::scale(0.0, 1.0));
        let bounds = PxRect::new(ui_draw::geometry::PxPoint::new(0.0, 0.0), PxSize::new(1.0, 1.0));
        let error = pack_image(7, &item, &bounds, 0, &mut Vec::new()).unwrap_err();
        assert!(
            matches!(
                error,
                UiRenderError::InvalidDrawList(DrawListError::DegenerateTransform { item: 7, .. })
            ),
            "{error:?}"
        );
    }
}
