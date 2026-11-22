//! Миниатюра снимка: план размера, CPU-эталон и типы итога (сессия 14g, решения 154, 155, 165).
//!
//! Эталон здесь — ИСТОЧНИК ПРАВДЫ формулы; шейдер `photo_thumbnail.comp` повторяет её (одна рабочая группа на
//! пиксель миниатюры), а паритет держат тесты с явным допуском. GPU-часть (проход, цели, запись команд) — в `thumbnail_pass`.
//!
//! # Правило охвата (footprint)
//! Миниатюра строится в ПОВЁРНУТОМ пространстве («как покажет галерея»): сначала снимок поворачивается по
//! часовой стрелке на `StillRotation`, затем уменьшается. Пиксель миниатюры `(ox, oy)` покрывает в повёрнутом
//! пространстве коробку `[ox·n/m, (ox+1)·n/m) × [oy·n'/m', (oy+1)·n'/m')`, где `n × n'` — размер повёрнутого
//! снимка, `m × m'` — размер миниатюры. Вес повёрнутого пикселя `(u, v)` — ПЛОЩАДЬ пересечения его клетки
//! `[u, u+1) × [v, v+1)` с коробкой (честное усреднение по площади, а не выборка: уменьшение 16:1 не даёт
//! муара). Дробные границы коробки (плавный зум даёт произвольные размеры) получают дробные веса.
//!
//! Вес считается ЦЕЛЫМИ числами в единицах `1/m` (клетка `[u·m, (u+1)·m)`, коробка `[ox·n, (ox+1)·n)`):
//! пересечение — целое, поэтому CPU и GPU видят одни и те же веса бит-в-бит, а сумма весов по оси равна `n`.
//!
//! # Свет
//! Усреднение — в ЛИНЕЙНОМ свете: sRGB → линейный → взвешенное среднее → sRGB → округление «половина вверх»
//! (как у квантования основного снимка). Среднее в закодированных значениях темнило бы мелкие светлые детали
//! (чёрно-белая шашка стала бы серой 128, а не 188).
//!
//! # Размер
//! Длинная сторона миниатюры = `long_side_px` (или длинная сторона снимка, если она меньше: увеличивать нечего),
//! короткая — пропорциональная, округлённая «половина вверх», не меньше 1.

use camera_contract::geometry::PixelSize;
use still_contract::shot::StillRotation;

use super::error::PhotoError;

/// Байтов на пиксель RGBA8.
const BYTES_PER_PIXEL: usize = 4;

/// Размер повёрнутого снимка: при 90° и 270° ширина и высота меняются местами.
pub fn rotated_size(size: PixelSize, rotation: StillRotation) -> PixelSize {
    match rotation {
        StillRotation::None | StillRotation::Clockwise180 => size,
        StillRotation::Clockwise90 | StillRotation::Clockwise270 => PixelSize::new(size.height, size.width),
    }
}

/// Код поворота для шейдера (`pass_mode`): 0, 1, 2, 3 — четверти оборота по часовой стрелке.
pub(crate) fn rotation_code(rotation: StillRotation) -> u32 {
    match rotation {
        StillRotation::None => 0,
        StillRotation::Clockwise90 => 1,
        StillRotation::Clockwise180 => 2,
        StillRotation::Clockwise270 => 3,
    }
}

/// Пиксель снимка (в ориентации сенсора), который попадает в повёрнутый пиксель `(u, v)`.
///
/// `source` — размер снимка ДО поворота. Для 90° верхний левый угол снимка уходит в верхний правый угол
/// повёрнутой картинки (так же поворачивает галерея по тегу `Orientation`/`irot`).
pub(crate) fn source_texel(rotation: StillRotation, u: u32, v: u32, source: PixelSize) -> (u32, u32) {
    match rotation {
        StillRotation::None => (u, v),
        StillRotation::Clockwise90 => (v, source.height - 1 - u),
        StillRotation::Clockwise180 => (source.width - 1 - u, source.height - 1 - v),
        StillRotation::Clockwise270 => (source.width - 1 - v, u),
    }
}

/// План миниатюры одного снимка: откуда и какого размера получается картинка. Размер — функция трёх чисел
/// (размер снимка, поворот, длинная сторона), поэтому цели (выделяются заранее) и проход (записывается
/// позже) сходятся на одном плане, а расхождение ловится сравнением планов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThumbnailPlan {
    source_size: PixelSize,
    rotation: StillRotation,
    size: PixelSize,
}

impl ThumbnailPlan {
    /// Строит план. Пустой снимок или нулевая длинная сторона — отказ (миниатюре не из чего/во что).
    pub fn new(source_size: PixelSize, rotation: StillRotation, long_side_px: u32) -> Result<Self, PhotoError> {
        if source_size.width == 0 || source_size.height == 0 || long_side_px == 0 {
            return Err(PhotoError::InvalidInput {
                what: format!("миниатюра {source_size:?} в {long_side_px} px: нужны ненулевые размеры"),
            });
        }
        let rotated = rotated_size(source_size, rotation);
        let size = fit_long_side(rotated, long_side_px);
        Ok(Self {
            source_size,
            rotation,
            size,
        })
    }

    /// Размер миниатюры (уже повёрнутой).
    pub fn size(&self) -> PixelSize {
        self.size
    }

    /// Размер снимка до поворота.
    pub fn source_size(&self) -> PixelSize {
        self.source_size
    }

    /// Поворот снимка.
    pub fn rotation(&self) -> StillRotation {
        self.rotation
    }

    /// Размер повёрнутого снимка (пространство, в котором считается охват).
    pub fn rotated_source_size(&self) -> PixelSize {
        rotated_size(self.source_size, self.rotation)
    }

    /// Сколько байт занимает миниатюра плотными строками RGBA.
    pub fn byte_len(&self) -> u64 {
        self.size.pixel_count() * BYTES_PER_PIXEL as u64
    }
}

/// Вписывает размер в длинную сторону `long_side_px`: без увеличения, короткая сторона — «половина вверх», ≥ 1.
fn fit_long_side(size: PixelSize, long_side_px: u32) -> PixelSize {
    let long = size.width.max(size.height);
    if long <= long_side_px {
        return size;
    }
    let scaled = |side: u32| -> u32 {
        // round(side · L / long) целыми числами: (2·side·L + long) / (2·long).
        let numerator = 2 * u64::from(side) * u64::from(long_side_px) + u64::from(long);
        let value = numerator / (2 * u64::from(long));
        u32::try_from(value).unwrap_or(u32::MAX).max(1)
    };
    if size.width >= size.height {
        PixelSize::new(long_side_px, scaled(size.height))
    } else {
        PixelSize::new(scaled(size.width), long_side_px)
    }
}

/// Длина пересечения клетки `[index·m, (index+1)·m)` с коробкой `[out·n, (out+1)·n)` (единицы `1/m`).
fn overlap(index: u32, out: u32, source_len: u32, out_len: u32) -> u64 {
    let cell_begin = u64::from(index) * u64::from(out_len);
    let cell_end = cell_begin + u64::from(out_len);
    let box_begin = u64::from(out) * u64::from(source_len);
    let box_end = box_begin + u64::from(source_len);
    let begin = cell_begin.max(box_begin);
    let end = cell_end.min(box_end);
    end.saturating_sub(begin)
}

/// Диапазон повёрнутых пикселей, которых касается коробка: `[floor(out·n/m), ceil((out+1)·n/m))`.
fn cell_range(out: u32, source_len: u32, out_len: u32) -> std::ops::Range<u32> {
    let first = u64::from(out) * u64::from(source_len) / u64::from(out_len);
    let end = (u64::from(out) + 1) * u64::from(source_len);
    let last_exclusive = end.div_ceil(u64::from(out_len));
    let clamp = |value: u64| u32::try_from(value).unwrap_or(u32::MAX).min(source_len);
    clamp(first)..clamp(last_exclusive)
}

/// sRGB → линейный свет (IEC 61966-2-1), вход 0…1.
pub(crate) fn srgb_to_linear(encoded: f32) -> f32 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// Линейный свет → sRGB, вход 0…1.
pub(crate) fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// CPU-эталон миниатюры: RGBA8 sRGB `size`, поворот `rotation`, длинная сторона `long_side_px`.
/// Возвращает размер миниатюры и её байты (RGBA построчно, альфа 255).
///
/// Формулы и веса — те же, что у шейдера (веса целые, точные). Порядок суммирования другой (шейдер складывает
/// частичные суммы нитей группы деревом, эталон — подряд): расхождение — единицы ULP f32 и `pow` видеокарты,
/// то есть редкое округление в соседнюю градацию (допуск в тестах паритета — 1 градация).
pub fn reference_thumbnail(
    rgba: &[u8],
    size: PixelSize,
    rotation: StillRotation,
    long_side_px: u32,
) -> Result<(PixelSize, Vec<u8>), PhotoError> {
    let plan = ThumbnailPlan::new(size, rotation, long_side_px)?;
    let expected_len = usize::try_from(size.pixel_count() * BYTES_PER_PIXEL as u64).unwrap_or(usize::MAX);
    if rgba.len() != expected_len {
        return Err(PhotoError::InvalidInput {
            what: format!(
                "эталон миниатюры: {} байт, а снимок {size:?} занимает {expected_len}",
                rgba.len()
            ),
        });
    }
    let rotated = plan.rotated_source_size();
    let target = plan.size();
    let mut out = Vec::with_capacity(plan.byte_len() as usize);
    for target_y in 0..target.height {
        for target_x in 0..target.width {
            let mut sum = [0.0_f32; 3];
            for v in cell_range(target_y, rotated.height, target.height) {
                let weight_y = overlap(v, target_y, rotated.height, target.height);
                for u in cell_range(target_x, rotated.width, target.width) {
                    let weight = (overlap(u, target_x, rotated.width, target.width) * weight_y) as f32;
                    let (x, y) = source_texel(rotation, u, v, size);
                    let offset = (y as usize * size.width as usize + x as usize) * BYTES_PER_PIXEL;
                    for (channel, accumulator) in sum.iter_mut().enumerate() {
                        let linear = srgb_to_linear(f32::from(rgba[offset + channel]) / 255.0);
                        *accumulator += weight * linear;
                    }
                }
            }
            let norm = rotated.width as f32 * rotated.height as f32;
            for accumulator in sum {
                let encoded = linear_to_srgb(accumulator / norm);
                out.push((encoded.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8);
            }
            out.push(255);
        }
    }
    Ok((target, out))
}

/// Почему миниатюры нет. Отдельные варианты, чтобы рендер писал в лог причину, а не «не вышло».
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbnailUnavailable {
    /// Пайплайн уменьшения не создался (проявитель работает без миниатюр).
    PipelineMissing,
    /// Цели миниатюры (картинка, буфер копии) не выделились.
    TargetsNotAllocated(String),
    /// Проход не принят проверкой или привязка не удалась до записи команд.
    NotRecorded(String),
    /// Буфер копии не запечатан доказательством последней полосы (GPU мог ещё писать).
    SealRefused(String),
    /// Миниатюру уже забрали ([`super::DevelopedPhoto::take_thumbnail`] отдаёт её один раз).
    AlreadyTaken,
}

impl std::fmt::Display for ThumbnailUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PipelineMissing => write!(formatter, "пайплайн уменьшения не создан"),
            Self::TargetsNotAllocated(what) => write!(formatter, "цели миниатюры не выделены: {what}"),
            Self::NotRecorded(what) => write!(formatter, "миниатюра не записана в команды: {what}"),
            Self::SealRefused(what) => write!(formatter, "буфер миниатюры не запечатан: {what}"),
            Self::AlreadyTaken => write!(formatter, "миниатюру уже забрали"),
        }
    }
}

/// Итог [`super::DevelopedPhoto::take_thumbnail`]: миниатюра в ОБЫЧНОЙ памяти (ни одного объекта GPU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbnailOutcome {
    /// Готова: RGBA8 sRGB построчно, повёрнута «как покажет галерея», без зеркала.
    Ready {
        /// Размер уже повёрнутой миниатюры.
        size: PixelSize,
        /// Байты RGBA (`size.width × size.height × 4`).
        rgba: Vec<u8>,
    },
    /// Нет; проявление от этого не страдает.
    Unavailable(ThumbnailUnavailable),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Картинка из функции «пиксель → RGBA».
    fn image(size: PixelSize, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for y in 0..size.height {
            for x in 0..size.width {
                bytes.extend_from_slice(&pixel(x, y));
            }
        }
        bytes
    }

    fn pixel_at(bytes: &[u8], size: PixelSize, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * size.width as usize + x as usize) * 4;
        [bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]
    }

    const MARK: [u8; 4] = [255, 0, 0, 255];
    const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

    #[test]
    fn marked_corner_lands_where_the_gallery_shows_it_for_every_rotation() {
        // Снимок 4×2, метка в левом верхнем углу (0, 0). Длинная сторона 100 — без уменьшения.
        let size = PixelSize::new(4, 2);
        let source = image(size, |x, y| if (x, y) == (0, 0) { MARK } else { BACKGROUND });
        // (поворот, размер результата, куда уходит метка): верхний левый угол при повороте по часовой стрелке
        // идёт 0° → левый верх, 90° → правый верх, 180° → правый низ, 270° → левый низ.
        let cases = [
            (StillRotation::None, PixelSize::new(4, 2), (0, 0)),
            (StillRotation::Clockwise90, PixelSize::new(2, 4), (1, 0)),
            (StillRotation::Clockwise180, PixelSize::new(4, 2), (3, 1)),
            (StillRotation::Clockwise270, PixelSize::new(2, 4), (0, 3)),
        ];
        for (rotation, expected_size, (mark_x, mark_y)) in cases {
            let (out_size, out) = reference_thumbnail(&source, size, rotation, 100).unwrap();
            assert_eq!(out_size, expected_size, "{rotation:?}");
            for y in 0..out_size.height {
                for x in 0..out_size.width {
                    let expected = if (x, y) == (mark_x, mark_y) { MARK } else { BACKGROUND };
                    assert_eq!(pixel_at(&out, out_size, x, y), expected, "{rotation:?} в ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn rotation_is_not_a_mirror() {
        // Снимок 2×2: красный сверху-слева, зелёный сверху-справа. После 90° по часовой красный сверху-справа,
        // зелёный — снизу-справа; зеркало дало бы красный сверху-слева.
        let size = PixelSize::new(2, 2);
        let source = image(size, |x, y| match (x, y) {
            (0, 0) => [255, 0, 0, 255],
            (1, 0) => [0, 255, 0, 255],
            _ => BACKGROUND,
        });
        let (out_size, out) = reference_thumbnail(&source, size, StillRotation::Clockwise90, 100).unwrap();
        assert_eq!(pixel_at(&out, out_size, 1, 0), [255, 0, 0, 255]);
        assert_eq!(pixel_at(&out, out_size, 1, 1), [0, 255, 0, 255]);
        assert_eq!(pixel_at(&out, out_size, 0, 0), BACKGROUND);
    }

    #[test]
    fn checkerboard_averages_in_linear_light_not_in_encoded_values() {
        // Шашка 2×2 «чёрный/белый», уменьшение в один пиксель: линейное среднее 0,5 → sRGB 0,7354 → 188.
        // Среднее закодированных значений дало бы 127–128.
        let size = PixelSize::new(2, 2);
        let source = image(size, |x, y| {
            if (x + y) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                BACKGROUND
            }
        });
        // Длинная сторона 1: миниатюра 1×1 покрывает весь снимок.
        let (out_size, out) = reference_thumbnail(&source, size, StillRotation::None, 1).unwrap();
        assert_eq!(out_size, PixelSize::new(1, 1));
        assert_eq!(out, vec![188, 188, 188, 255]);
    }

    #[test]
    fn fractional_ratio_weights_edge_pixels_by_covered_area() {
        // 3 пикселя в 2: границы коробок 0…1,5 и 1,5…3. Средний пиксель делится пополам между коробками.
        // Строка: чёрный, белый, чёрный → обе коробки получают линейно 0,5/1,5 = 1/3 света.
        let size = PixelSize::new(3, 1);
        let source = image(size, |x, _| if x == 1 { [255, 255, 255, 255] } else { BACKGROUND });
        let (out_size, out) = reference_thumbnail(&source, size, StillRotation::None, 2).unwrap();
        assert_eq!(out_size, PixelSize::new(2, 1));
        let expected = (linear_to_srgb(1.0 / 3.0) * 255.0 + 0.5).floor() as u8;
        assert_eq!(pixel_at(&out, out_size, 0, 0)[0], expected);
        assert_eq!(pixel_at(&out, out_size, 1, 0)[0], expected);
        assert_ne!(expected, 0);
    }

    #[test]
    fn flat_colour_is_preserved_by_any_reduction() {
        let size = PixelSize::new(37, 23);
        let source = image(size, |_, _| [200, 100, 30, 255]);
        for rotation in [StillRotation::None, StillRotation::Clockwise90] {
            let (_, out) = reference_thumbnail(&source, size, rotation, 7).unwrap();
            for pixel in out.chunks_exact(4) {
                assert_eq!(pixel, [200, 100, 30, 255]);
            }
        }
    }

    #[test]
    fn plan_sizes_follow_the_long_side_with_rounded_short_side() {
        let plan = |width, height, rotation, long| {
            ThumbnailPlan::new(PixelSize::new(width, height), rotation, long)
                .unwrap()
                .size()
        };
        assert_eq!(plan(4096, 3072, StillRotation::None, 256), PixelSize::new(256, 192));
        // Портрет: поворот на 90° делает 4096×3072 → 3072×4096, длинная сторона — высота.
        assert_eq!(
            plan(4096, 3072, StillRotation::Clockwise90, 256),
            PixelSize::new(192, 256)
        );
        // Округление «половина вверх»: 1001·256/4000 = 64,06 → 64; 1000·256/4000 = 64,0.
        assert_eq!(plan(4000, 1001, StillRotation::None, 256), PixelSize::new(256, 64));
        // Узкая полоса не обнуляется.
        assert_eq!(plan(8000, 3, StillRotation::None, 256), PixelSize::new(256, 1));
        // Меньше лимита — не увеличиваем.
        assert_eq!(plan(100, 50, StillRotation::None, 256), PixelSize::new(100, 50));
        // Произвольный размер плавного зума.
        assert_eq!(plan(3333, 2501, StillRotation::None, 256), PixelSize::new(256, 192));
    }

    #[test]
    fn empty_input_and_zero_long_side_are_refused() {
        let ok = PixelSize::new(4, 4);
        assert!(ThumbnailPlan::new(PixelSize::new(0, 4), StillRotation::None, 8).is_err());
        assert!(ThumbnailPlan::new(ok, StillRotation::None, 0).is_err());
        assert!(reference_thumbnail(&[0; 10], ok, StillRotation::None, 8).is_err());
    }
}
