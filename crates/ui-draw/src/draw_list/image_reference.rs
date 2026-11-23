//! Эталонный цвет пикселя картинки (`DrawItem::Image`) «в форме шейдера» (сессии 14g, 14h).
//!
//! `ui-render-vulkan` сверяет шейдер `ui_image` с [`shade_image_pixel`] / [`shade_image_pixel_with_live`].
//! Порядок один и тот же:
//! 1. обратное преобразование: точка экрана → локальная точка рамки (центр рамки — ноль);
//! 2. покрытие формы обрезки (круг или прямоугольник) — то же поле расстояний и то же сглаживание края, что у фигур;
//! 3. отображение `Cover`/`Stretch`: локальная точка → координата в источнике ([`fit_uv_scale`]);
//! 4. билинейная выборка по значениям источника: центры текселей в +0,5, края зажаты (как у сэмплера GPU);
//! 5. перевод цвета источника в цвет экрана ([`srgb_picture_to_display`]) — ТОЛЬКО для картинки из памяти;
//!    живая картинка уже в цвете экрана ([`ImageColorSpace::DisplayEncoded`]);
//! 6. предумножение: `альфа источника (прямая) × покрытие × прозрачность элемента`.

use crate::color::{DisplayPrimaries, PremultipliedColor, srgb_picture_to_display};
use crate::draw_list::image_source::{ImageColorSpace, ImageSource, LiveImageTexels};
use crate::draw_list::items::{ImageClip, ImageFit, ImageItem};
use crate::draw_list::pixel_reference::shape_distances;
use crate::geometry::{PxPoint, PxSize};
use crate::picture::{PictureSize, UiPicture};
use crate::shape::{CornerSpec, ShapeGeometry, coverage};
use crate::transform::AffineError;

/// Диаметр круга, вписанного в рамку (меньшая сторона).
pub fn clip_circle_diameter(frame: PxSize) -> f32 {
    frame.min_side()
}

/// Геометрия формы обрезки картинки в локальных координатах рамки. Шейдер получает её параметры SDF
/// из той же геометрии, поэтому форма у эталона и GPU одна.
pub fn clip_geometry(clip: ImageClip, frame: PxSize) -> ShapeGeometry {
    match clip {
        ImageClip::Circle => ShapeGeometry::Circle {
            diameter: clip_circle_diameter(frame),
        },
        // Прямоугольник рамки: «скруглённый ящик» с нулевым радиусом (угол — острый).
        ImageClip::None => ShapeGeometry::RoundedRect {
            size: frame,
            corner: CornerSpec::circular(0.0),
        },
    }
}

/// Сколько единиц `uv` источника приходится на один локальный пиксель рамки по каждой оси.
///
/// * `Cover`: масштаб `s = max(рамка.ш / источник.ш, рамка.в / источник.в)` (пикселей рамки на пиксель
///   источника) — картинка закрывает рамку целиком, центр картинки лежит в центре рамки, лишнее по длинной
///   стороне обрезается поровну.
/// * `Stretch`: `1 / размер рамки` по каждой оси — размер источника не важен, края источника = края рамки.
///
/// Координата источника для локальной точки `p`: `uv = 0,5 + p · результат`.
pub fn fit_uv_scale(fit: ImageFit, frame: PxSize, source: PictureSize) -> [f32; 2] {
    match fit {
        ImageFit::Cover => {
            let (source_width, source_height) = (source.width as f32, source.height as f32);
            let scale = (frame.width / source_width).max(frame.height / source_height);
            [1.0 / (scale * source_width), 1.0 / (scale * source_height)]
        }
        ImageFit::Stretch => [1.0 / frame.width, 1.0 / frame.height],
    }
}

/// Билинейная выборка в точке `uv` (0..1 по картинке) по функции `fetch(столбец, строка)`: значения 0..1,
/// прямая (не предумноженная) альфа. Центры текселей — в +0,5 текселя, края зажаты. Общая для картинок из
/// памяти и живых: арифметика одна и та же.
fn sample_bilinear(size: PictureSize, uv: [f32; 2], fetch: impl Fn(u32, u32) -> [f32; 4]) -> [f32; 4] {
    let x = uv[0] * size.width as f32 - 0.5;
    let y = uv[1] * size.height as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    // Зажим по краям: `as i64` у floor безопасен (значения — порядка размера картинки).
    let clamped_fetch = |tx: i64, ty: i64| -> [f32; 4] {
        let column = tx.clamp(0, i64::from(size.width) - 1) as u32;
        let row = ty.clamp(0, i64::from(size.height) - 1) as u32;
        fetch(column, row)
    };
    let (ix, iy) = (x0 as i64, y0 as i64);
    let (top_left, top_right) = (clamped_fetch(ix, iy), clamped_fetch(ix + 1, iy));
    let (bottom_left, bottom_right) = (clamped_fetch(ix, iy + 1), clamped_fetch(ix + 1, iy + 1));
    std::array::from_fn(|channel| {
        let top = top_left[channel] * (1.0 - fx) + top_right[channel] * fx;
        let bottom = bottom_left[channel] * (1.0 - fx) + bottom_right[channel] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

/// Выборка картинки из памяти: байты → 0..1.
fn sample_picture(picture: &UiPicture, uv: [f32; 2]) -> [f32; 4] {
    sample_bilinear(picture.size(), uv, |column, row| {
        // Координаты уже в пределах картинки: `None` недостижим, прозрачный чёрный — честный запасной вариант.
        picture
            .texel(column, row)
            .map_or([0.0; 4], |texel| texel.map(|byte| f32::from(byte) / 255.0))
    })
}

/// Выборка живой картинки: пиксели уже 0..1.
fn sample_live(texels: &LiveImageTexels<'_>, uv: [f32; 2]) -> [f32; 4] {
    sample_bilinear(texels.size(), uv, |column, row| {
        texels.texel(column, row).unwrap_or([0.0; 4])
    })
}

/// Источник, по которому реально можно выбирать пиксели (живой — только если вызывающий дал пиксели).
enum ResolvedSource<'a> {
    Picture(&'a UiPicture),
    Live(&'a LiveImageTexels<'a>),
}

impl ResolvedSource<'_> {
    fn size(&self) -> PictureSize {
        match self {
            Self::Picture(picture) => picture.size(),
            Self::Live(texels) => texels.size(),
        }
    }

    fn sample(&self, uv: [f32; 2]) -> [f32; 4] {
        match self {
            Self::Picture(picture) => sample_picture(picture, uv),
            Self::Live(texels) => sample_live(texels, uv),
        }
    }
}

/// Цвет пикселя картинки ИЗ ПАМЯТИ (premultiplied) в центре пикселя `screen_point`.
/// Для элемента с живым источником — прозрачный пиксель (так рендер поступает, когда живой картинки
/// на кадре нет); живые картинки считает [`shade_image_pixel_with_live`].
///
/// `primaries` — основные цвета экрана (из них шейдер и эталон берут матрицу перевода), `aa_width_px` —
/// ширина сглаживания края формы (в шейдере 1 px). Ошибка — вырожденное преобразование элемента.
pub fn shade_image_pixel(
    item: &ImageItem,
    screen_point: PxPoint,
    primaries: DisplayPrimaries,
    aa_width_px: f32,
) -> Result<PremultipliedColor, AffineError> {
    shade_image_pixel_with_live(item, screen_point, primaries, aa_width_px, None)
}

/// То же, но с пикселями живой картинки от вызывающего (`live` — то, что в этом кадре лежит в видеопамяти).
/// `None` при живом источнике = «картинки на кадре нет»: прозрачный пиксель. Для источника из памяти
/// `live` не используется.
pub fn shade_image_pixel_with_live(
    item: &ImageItem,
    screen_point: PxPoint,
    primaries: DisplayPrimaries,
    aa_width_px: f32,
    live: Option<&LiveImageTexels<'_>>,
) -> Result<PremultipliedColor, AffineError> {
    let inverse = item.transform.inverse()?;
    let local = inverse.apply_point(screen_point);

    let clip = clip_geometry(item.clip, item.size_px);
    let clip_distance = shape_distances(&clip, &inverse, screen_point).screen_px;
    let clip_coverage = coverage(clip_distance, aa_width_px);

    // Источник: размер (для `Cover`) и выборка. Живая картинка без пикселей — рисовать нечего.
    let resolved = match (&item.source, live) {
        (ImageSource::Picture(picture), _) => ResolvedSource::Picture(picture),
        (ImageSource::Live(_), Some(texels)) => ResolvedSource::Live(texels),
        (ImageSource::Live(_), None) => return Ok(PremultipliedColor::TRANSPARENT),
    };
    let uv_scale = fit_uv_scale(item.fit, item.size_px, resolved.size());
    let uv = [0.5 + local.x * uv_scale[0], 0.5 + local.y * uv_scale[1]];
    let texel = resolved.sample(uv);
    let rgb = match item.source.color_space() {
        ImageColorSpace::SrgbPicture => srgb_picture_to_display([texel[0], texel[1], texel[2]], primaries),
        // Живая картинка уже в цвете экрана: НИКАКОГО перевода (иначе на P3-экране цвет поплыл бы второй раз).
        ImageColorSpace::DisplayEncoded => [texel[0], texel[1], texel[2]],
    };

    let alpha = texel[3] * clip_coverage * item.opacity.get();
    Ok(PremultipliedColor {
        r: rgb[0] * alpha,
        g: rgb[1] * alpha,
        b: rgb[2] * alpha,
        a: alpha,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, reason = "тесты")]

    use super::*;
    use crate::color::Opacity;
    use crate::picture::PictureEncoding;
    use crate::transform::Affine2;

    const AA: f32 = 1.0;
    const EPS: f32 = 1.0e-5;

    fn picture_from(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> UiPicture {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&texel(x, y));
            }
        }
        UiPicture::from_rgba8(PictureSize::new(width, height), PictureEncoding::Srgb, bytes).unwrap()
    }

    /// Картинка 40×20, красный канал каждого столбца = номер столбца × 6 (все столбцы различимы).
    fn column_ramp() -> UiPicture {
        picture_from(40, 20, |x, _| [(x * 6) as u8, 0, 0, 255])
    }

    fn item(picture: UiPicture, frame: (f32, f32), transform: Affine2) -> ImageItem {
        ImageItem {
            source: picture.into(),
            transform,
            size_px: PxSize::new(frame.0, frame.1),
            fit: ImageFit::Cover,
            clip: ImageClip::Circle,
            opacity: Opacity::OPAQUE,
        }
    }

    fn shade(item: &ImageItem, x: f32, y: f32, primaries: DisplayPrimaries) -> PremultipliedColor {
        shade_image_pixel(item, PxPoint::new(x, y), primaries, AA).unwrap()
    }

    #[test]
    fn cover_cuts_the_long_side_equally_and_keeps_the_middle_columns() {
        // Рамка 20×20 над источником 40×20: масштаб 1 пиксель на пиксель, видны столбцы 10..30, обрезаны 0..10 и 30..40.
        let item = item(column_ramp(), (20.0, 20.0), Affine2::translate(50.0, 50.0));
        // Крайние видимые пиксели рамки по горизонтали (круг на оси y = 0 доходит до края: покрытие 1).
        let left = shade(&item, 40.5, 50.0, DisplayPrimaries::Srgb);
        let right = shade(&item, 59.5, 50.0, DisplayPrimaries::Srgb);
        assert!(
            (left.r - 60.0 / 255.0).abs() < EPS,
            "слева первый видимый столбец 10: {left:?}"
        );
        assert!(
            (right.r - 174.0 / 255.0).abs() < EPS,
            "справа последний видимый столбец 29: {right:?}"
        );
        assert!((left.a - 1.0).abs() < EPS && (right.a - 1.0).abs() < EPS);
    }

    #[test]
    fn sampling_interpolates_encoded_values_between_texel_centres() {
        // Локальный x = −9 → позиция в источнике ровно между центрами столбцов 10 и 11: среднее ЗАКОДИРОВАННЫХ (60 и 66).
        let item = item(column_ramp(), (20.0, 20.0), Affine2::translate(50.0, 50.0));
        let middle = shade(&item, 41.0, 50.0, DisplayPrimaries::Srgb);
        assert!((middle.r - 63.0 / 255.0).abs() < EPS, "{middle:?}");
    }

    #[test]
    fn circle_edge_has_half_coverage_and_the_frame_corners_are_empty() {
        let item = item(column_ramp(), (20.0, 20.0), Affine2::translate(50.0, 50.0));
        let centre = shade(&item, 50.0, 50.0, DisplayPrimaries::Srgb);
        assert!((centre.a - 1.0).abs() < EPS, "{centre:?}");
        // Ровно на границе круга (радиус 10, вправо от центра): половина покрытия.
        let on_edge = shade(&item, 60.0, 50.0, DisplayPrimaries::Srgb);
        assert!((on_edge.a - 0.5).abs() < 1.0e-3, "{on_edge:?}");
        // Угол рамки лежит вне вписанного круга: ничего не нарисовано.
        let corner = shade(&item, 59.5, 59.5, DisplayPrimaries::Srgb);
        assert_eq!(corner, PremultipliedColor::TRANSPARENT);
    }

    #[test]
    fn circle_of_a_wide_frame_is_inscribed_into_the_short_side() {
        let item = item(column_ramp(), (40.0, 20.0), Affine2::translate(50.0, 50.0));
        // Вдоль длинной стороны рамка шире круга: x = 50 + 15 уже вне круга радиуса 10.
        assert_eq!(shade(&item, 65.0, 50.0, DisplayPrimaries::Srgb).a, 0.0);
        assert!(shade(&item, 55.0, 50.0, DisplayPrimaries::Srgb).a > 0.99);
    }

    #[test]
    fn opacity_and_source_alpha_multiply_and_colour_is_premultiplied() {
        let half_transparent = picture_from(2, 2, |_, _| [200, 100, 50, 128]);
        let mut item = item(half_transparent, (20.0, 20.0), Affine2::translate(50.0, 50.0));
        item.opacity = Opacity::new(0.5).unwrap();
        let pixel = shade(&item, 50.0, 50.0, DisplayPrimaries::Srgb);
        let alpha = 128.0 / 255.0 * 0.5;
        assert!((pixel.a - alpha).abs() < EPS, "{pixel:?}");
        assert!((pixel.r - 200.0 / 255.0 * alpha).abs() < EPS, "{pixel:?}");
        assert!((pixel.g - 100.0 / 255.0 * alpha).abs() < EPS, "{pixel:?}");
        assert!((pixel.b - 50.0 / 255.0 * alpha).abs() < EPS, "{pixel:?}");
    }

    #[test]
    fn display_p3_makes_saturated_red_numerically_less_red_and_leaves_grey_alone() {
        let red = picture_from(2, 2, |_, _| [255, 0, 0, 255]);
        let red_item = item(red, (20.0, 20.0), Affine2::translate(50.0, 50.0));
        let in_srgb = shade(&red_item, 50.0, 50.0, DisplayPrimaries::Srgb);
        let in_p3 = shade(&red_item, 50.0, 50.0, DisplayPrimaries::DisplayP3);
        assert_eq!((in_srgb.r, in_srgb.g, in_srgb.b), (1.0, 0.0, 0.0));
        assert!(in_p3.r < 0.93 && in_p3.r > 0.90, "{in_p3:?}");
        assert!(
            in_p3.g > 0.15 && in_p3.b > 0.1,
            "в P3 у красного sRGB появляются G и B: {in_p3:?}"
        );

        let grey = picture_from(2, 2, |_, _| [128, 128, 128, 255]);
        let grey_item = item(grey, (20.0, 20.0), Affine2::translate(50.0, 50.0));
        let grey_p3 = shade(&grey_item, 50.0, 50.0, DisplayPrimaries::DisplayP3);
        for channel in [grey_p3.r, grey_p3.g, grey_p3.b] {
            assert!((channel - 128.0 / 255.0).abs() < 1.0e-4, "{grey_p3:?}");
        }
    }

    #[test]
    fn quarter_turn_moves_the_crop_axis_from_screen_x_to_screen_y() {
        // Поворот на 90° вокруг центра: ось «лево–право» картинки становится «вверх–вниз» на экране.
        let transform = Affine2::rotate(std::f32::consts::FRAC_PI_2).then(Affine2::translate(50.0, 50.0));
        let item = item(column_ramp(), (20.0, 20.0), transform);
        let along_y_a = shade(&item, 50.0, 41.0, DisplayPrimaries::Srgb);
        let along_y_b = shade(&item, 50.0, 59.0, DisplayPrimaries::Srgb);
        let along_x_a = shade(&item, 41.0, 50.0, DisplayPrimaries::Srgb);
        let along_x_b = shade(&item, 59.0, 50.0, DisplayPrimaries::Srgb);
        assert!(
            (along_y_a.r - along_y_b.r).abs() > 0.3,
            "вдоль экранной y цвет меняется: {along_y_a:?} {along_y_b:?}"
        );
        assert!(
            (along_x_a.r - along_x_b.r).abs() < EPS,
            "вдоль экранной x цвет постоянен"
        );
    }

    #[test]
    fn degenerate_transform_is_an_error_not_a_blank_pixel() {
        let item = item(column_ramp(), (20.0, 20.0), Affine2::scale(0.0, 1.0));
        assert!(shade_image_pixel(&item, PxPoint::new(1.0, 1.0), DisplayPrimaries::Srgb, AA).is_err());
    }

    #[test]
    fn cover_scale_follows_the_limiting_side() {
        // Рамка 20×20, источник 40×20: ось x растянута вдвое меньше оси y.
        let scale = fit_uv_scale(ImageFit::Cover, PxSize::new(20.0, 20.0), PictureSize::new(40, 20));
        assert!(
            (scale[0] - 1.0 / 40.0).abs() < EPS && (scale[1] - 1.0 / 20.0).abs() < EPS,
            "{scale:?}"
        );
        // Высокий источник: ограничивает ширина.
        let tall = fit_uv_scale(ImageFit::Cover, PxSize::new(20.0, 20.0), PictureSize::new(10, 40));
        assert!(
            (tall[0] - 1.0 / 20.0).abs() < EPS && (tall[1] - 1.0 / 80.0).abs() < EPS,
            "{tall:?}"
        );
    }

    // ---- Живые картинки (14h) ----

    fn live_item(frame: (f32, f32), fit: ImageFit, clip: ImageClip, opacity: f32) -> ImageItem {
        ImageItem {
            source: ImageSource::Live(crate::draw_list::LiveImage::ColorScope),
            transform: Affine2::translate(50.0, 50.0),
            size_px: PxSize::new(frame.0, frame.1),
            fit,
            clip,
            opacity: Opacity::new(opacity).unwrap(),
        }
    }

    fn shade_live(
        item: &ImageItem,
        texels: &LiveImageTexels<'_>,
        x: f32,
        y: f32,
        primaries: DisplayPrimaries,
    ) -> PremultipliedColor {
        shade_image_pixel_with_live(item, PxPoint::new(x, y), primaries, AA, Some(texels)).unwrap()
    }

    #[test]
    fn stretch_maps_source_edges_to_frame_edges_whatever_the_source_size() {
        // Источник 2×1 в рамке 20×10: центры текселей на 1/4 и 3/4 ширины рамки, пропорции не сохраняются.
        let cells = [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
        let texels = LiveImageTexels::new(PictureSize::new(2, 1), &cells).unwrap();
        let item = live_item((20.0, 10.0), ImageFit::Stretch, ImageClip::None, 1.0);
        let left_quarter = shade_live(&item, &texels, 45.0, 50.0, DisplayPrimaries::Srgb);
        let right_quarter = shade_live(&item, &texels, 55.0, 50.0, DisplayPrimaries::Srgb);
        assert!(
            (left_quarter.r - 1.0).abs() < EPS && left_quarter.b.abs() < EPS,
            "{left_quarter:?}"
        );
        assert!(
            (right_quarter.b - 1.0).abs() < EPS && right_quarter.r.abs() < EPS,
            "{right_quarter:?}"
        );
        // Середина рамки — ровно среднее двух текселей (билинейно между центрами).
        let middle = shade_live(&item, &texels, 50.0, 50.0, DisplayPrimaries::Srgb);
        assert!(
            (middle.r - 0.5).abs() < EPS && (middle.b - 0.5).abs() < EPS,
            "{middle:?}"
        );
        let scale = fit_uv_scale(ImageFit::Stretch, PxSize::new(20.0, 10.0), PictureSize::new(2, 1));
        assert!(
            (scale[0] - 0.05).abs() < EPS && (scale[1] - 0.1).abs() < EPS,
            "{scale:?}"
        );
    }

    #[test]
    fn live_image_is_not_converted_to_display_p3_unlike_a_picture() {
        let cells = [[1.0, 0.0, 0.0, 1.0]; 4];
        let texels = LiveImageTexels::new(PictureSize::new(2, 2), &cells).unwrap();
        let item = live_item((20.0, 20.0), ImageFit::Stretch, ImageClip::None, 1.0);
        let live_p3 = shade_live(&item, &texels, 50.0, 50.0, DisplayPrimaries::DisplayP3);
        assert_eq!(
            (live_p3.r, live_p3.g, live_p3.b),
            (1.0, 0.0, 0.0),
            "цвет экрана остаётся как есть"
        );
        // Тот же «красный» из памяти (sRGB) на P3-экране уходит в другие числа.
        let red = picture_from(2, 2, |_, _| [255, 0, 0, 255]);
        let picture_p3 = shade(&item_with_source(red), 50.0, 50.0, DisplayPrimaries::DisplayP3);
        assert!(picture_p3.r < 0.93, "{picture_p3:?}");
    }

    fn item_with_source(picture: UiPicture) -> ImageItem {
        item(picture, (20.0, 20.0), Affine2::translate(50.0, 50.0))
    }

    #[test]
    fn live_alpha_is_straight_and_gets_premultiplied_with_opacity() {
        let cells = [[0.8, 0.4, 0.2, 0.5]; 4];
        let texels = LiveImageTexels::new(PictureSize::new(2, 2), &cells).unwrap();
        let item = live_item((20.0, 20.0), ImageFit::Stretch, ImageClip::None, 0.5);
        let pixel = shade_live(&item, &texels, 50.0, 50.0, DisplayPrimaries::Srgb);
        assert!((pixel.a - 0.25).abs() < EPS, "{pixel:?}");
        assert!(
            (pixel.r - 0.2).abs() < EPS && (pixel.g - 0.1).abs() < EPS && (pixel.b - 0.05).abs() < EPS,
            "{pixel:?}"
        );
    }

    #[test]
    fn rectangle_clip_covers_the_whole_frame_and_nothing_outside() {
        let cells = [[1.0; 4]; 4];
        let texels = LiveImageTexels::new(PictureSize::new(2, 2), &cells).unwrap();
        let item = live_item((20.0, 10.0), ImageFit::Stretch, ImageClip::None, 1.0);
        // Угловой пиксель рамки (центр на 0,5 px внутри обоих краёв) закрашен полностью — в отличие от круга.
        let corner = shade_live(&item, &texels, 40.5, 45.5, DisplayPrimaries::Srgb);
        assert!((corner.a - 1.0).abs() < 1.0e-3, "{corner:?}");
        // Ровно на границе — половина покрытия, за границей на 1 px — пусто.
        let on_edge = shade_live(&item, &texels, 60.0, 50.0, DisplayPrimaries::Srgb);
        assert!((on_edge.a - 0.5).abs() < 1.0e-3, "{on_edge:?}");
        assert_eq!(
            shade_live(&item, &texels, 61.0, 50.0, DisplayPrimaries::Srgb),
            PremultipliedColor::TRANSPARENT
        );
    }

    #[test]
    fn live_item_without_texels_draws_nothing() {
        let item = live_item((20.0, 20.0), ImageFit::Stretch, ImageClip::None, 1.0);
        let pixel = shade_image_pixel(&item, PxPoint::new(50.0, 50.0), DisplayPrimaries::Srgb, AA).unwrap();
        assert_eq!(pixel, PremultipliedColor::TRANSPARENT);
    }
}
