//! CPU-эталон тестового паттерна. ДОЛЖЕН совпадать с `shaders/test_pattern.frag` бит-в-бит.
//!
//! Формулу меняют только вместе с шейдером. Всё считается в целых числах `u32`.

use ash::vk;

use super::transform::SurfaceTransform;
use crate::error::GpuError;

/// Число клеток метки кадра (= число бит номера кадра, которые видны в метке).
pub const MARKER_CELLS: u32 = 8;
/// Полоса метки занимает `высота / MARKER_HEIGHT_DIVISOR` строк (минимум 1).
pub const MARKER_HEIGHT_DIVISOR: u32 = 8;
/// Шаг сдвига синего канала на номер кадра.
pub const BLUE_FRAME_STEP: u32 = 37;

/// Разрядность канала цели: от неё зависит `levels` — максимум значения канала.
///
/// Свойство «целочисленной точности»: значение канала `v` из `0..=levels` записывается как
/// `v / levels`, и UNORM-цель с `levels` ступенями возвращает ровно `v`. Для `Unorm8` это
/// 255 ступеней, readback сравнивается побитно. Для `Unorm10` — 1023 ступени, значения
/// `v / 1023` тоже точны, но 8-битного readback у такой цели нет: проверка идёт на уровне
/// целых «уровней» (`expected_levels`), без допуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    /// 8 бит на канал (RGBA8/BGRA8 UNORM): `levels = 255`.
    Unorm8,
    /// 10 бит на канал (A2B10G10R10/A2R10G10B10 UNORM): `levels = 1023`.
    Unorm10,
}

impl ColorDepth {
    /// Максимум значения канала.
    pub const fn levels(self) -> u32 {
        match self {
            ColorDepth::Unorm8 => 255,
            ColorDepth::Unorm10 => 1023,
        }
    }

    /// Разрядность по формату цели; sRGB и прочие форматы паттерн не поддерживает
    /// (sRGB-кодирование нарушило бы целочисленную точность).
    pub fn for_format(format: vk::Format) -> Result<Self, GpuError> {
        match format {
            vk::Format::R8G8B8A8_UNORM | vk::Format::B8G8R8A8_UNORM => Ok(ColorDepth::Unorm8),
            vk::Format::A2B10G10R10_UNORM_PACK32 | vk::Format::A2R10G10B10_UNORM_PACK32 => Ok(ColorDepth::Unorm10),
            other => Err(GpuError::Unsupported {
                what: format!("формат цели {other:?} для тестового паттерна (нужен UNORM8 или UNORM10)"),
            }),
        }
    }
}

/// Физические координаты пикселя → логические (прямые) и логический размер.
/// Формулы те же, что в шейдере; `quarter_turns` — 0..=3 по часовой.
pub fn physical_to_logical(x: u32, y: u32, extent: vk::Extent2D, quarter_turns: u32) -> (u32, u32, u32, u32) {
    let (width, height) = (extent.width, extent.height);
    match quarter_turns {
        1 => (height - 1 - y, x, height, width),
        2 => (width - 1 - x, height - 1 - y, width, height),
        3 => (y, width - 1 - x, height, width),
        _ => (x, y, width, height),
    }
}

/// Эталон в «уровнях» канала `[красный, зелёный, синий]` (каждый в `0..=levels`).
/// `x, y` — физические координаты пикселя цели размера `extent`.
pub fn expected_levels(
    x: u32,
    y: u32,
    extent: vk::Extent2D,
    frame: u32,
    transform: SurfaceTransform,
    depth: ColorDepth,
) -> Result<[u32; 3], GpuError> {
    let quarter_turns = transform.quarter_turns()?;
    let levels = depth.levels();
    let (logical_x, logical_y, logical_width, logical_height) = physical_to_logical(x, y, extent, quarter_turns);

    let marker_band_height = (logical_height / MARKER_HEIGHT_DIVISOR).max(1);
    if logical_y < marker_band_height {
        // Метка кадра: клетка светится, если соответствующий бит номера кадра равен 1.
        let cell = logical_x * MARKER_CELLS / logical_width;
        let bit = (frame >> cell) & 1;
        let level = bit * levels;
        return Ok([level, level, level]);
    }
    let red = logical_x * levels / (logical_width - 1).max(1);
    let green = logical_y * levels / (logical_height - 1).max(1);
    let diagonal = (logical_x + logical_y) * levels / (logical_width + logical_height - 2).max(1);
    let blue = (diagonal + frame * BLUE_FRAME_STEP) % (levels + 1);
    Ok([red, green, blue])
}

/// Эталон пикселя для прямой (`Identity`) цели RGBA8: `[r, g, b, a]`, `a = 255`.
pub fn expected_pixel(x: u32, y: u32, extent: vk::Extent2D, frame: u32) -> [u8; 4] {
    // Identity всегда поддержан, поэтому ошибки здесь невозможны; выражаем это значением, а не unwrap.
    expected_pixel_with_transform(x, y, extent, frame, SurfaceTransform::Identity).unwrap_or([0; 4])
}

/// Эталон пикселя RGBA8 для цели, повёрнутой как `transform`. Зеркальные варианты — `Unsupported`.
pub fn expected_pixel_with_transform(
    x: u32,
    y: u32,
    extent: vk::Extent2D,
    frame: u32,
    transform: SurfaceTransform,
) -> Result<[u8; 4], GpuError> {
    let [red, green, blue] = expected_levels(x, y, extent, frame, transform, ColorDepth::Unorm8)?;
    // Уровни UNORM8 лежат в 0..=255 по построению; saturating-преобразование документирует инвариант.
    let to_byte = |level: u32| u8::try_from(level).unwrap_or(u8::MAX);
    Ok([to_byte(red), to_byte(green), to_byte(blue), u8::MAX])
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTENT: vk::Extent2D = vk::Extent2D { width: 97, height: 61 };

    #[test]
    fn identity_gradient_corners_and_marker_are_as_documented() {
        // Нижний правый угол (вне полосы метки): красный и зелёный на максимуме.
        let [red, green, _, alpha] = expected_pixel(96, 60, EXTENT, 0);
        assert_eq!((red, green, alpha), (255, 255, 255));
        // Нижний левый: красный 0, зелёный 255.
        let [red, green, _, _] = expected_pixel(0, 60, EXTENT, 0);
        assert_eq!((red, green), (0, 255));
        // Полоса метки: высота 61/8 = 7 строк; кадр 0b1 → клетка 0 белая, остальные чёрные.
        assert_eq!(expected_pixel(0, 0, EXTENT, 1), [255, 255, 255, 255]);
        assert_eq!(expected_pixel(96, 0, EXTENT, 1), [0, 0, 0, 255]);
        assert_eq!(expected_pixel(0, 6, EXTENT, 1), [255, 255, 255, 255]);
        // Строка 7 — уже градиент, не метка.
        assert_ne!(expected_pixel(0, 7, EXTENT, 1)[0..2], [255, 255]);
    }

    #[test]
    fn frame_number_bits_select_marker_cells() {
        let cell_width = EXTENT.width / MARKER_CELLS;
        for frame in [0u32, 1, 5, 0b1010_0110, 255] {
            for cell in 0..MARKER_CELLS {
                let x = cell * EXTENT.width / MARKER_CELLS + cell_width / 2;
                let lit = expected_pixel(x, 0, EXTENT, frame)[0] == 255;
                assert_eq!(lit, (frame >> cell) & 1 == 1, "кадр {frame}, клетка {cell}");
            }
        }
    }

    #[test]
    fn rotation_maps_physical_pixels_to_upright_logical_ones() {
        // Для ROTATE_90 логическая ширина = физическая высота: верхняя полоса метки логически сверху,
        // физически это левый столбец (logical_y = physical_x → полоса при physical_x < band).
        let extent = vk::Extent2D { width: 61, height: 97 };
        let upright = vk::Extent2D { width: 97, height: 61 };
        for transform in SurfaceTransform::ROTATIONS {
            for frame in [0u32, 3] {
                for y in 0..extent.height {
                    for x in 0..extent.width {
                        let turns = transform.quarter_turns().unwrap();
                        let (logical_x, logical_y, lw, lh) = physical_to_logical(x, y, extent, turns);
                        let logical_extent = if transform.swaps_extent() { upright } else { extent };
                        assert_eq!((lw, lh), (logical_extent.width, logical_extent.height));
                        assert!(logical_x < lw && logical_y < lh);
                        let physical = expected_pixel_with_transform(x, y, extent, frame, transform).unwrap();
                        let upright_pixel = expected_pixel_with_transform(
                            logical_x,
                            logical_y,
                            logical_extent,
                            frame,
                            SurfaceTransform::Identity,
                        )
                        .unwrap();
                        assert_eq!(physical, upright_pixel, "{transform:?} ({x},{y})");
                    }
                }
            }
        }
    }

    #[test]
    fn rotation_is_a_bijection_of_pixels() {
        let extent = vk::Extent2D { width: 13, height: 7 };
        for turns in 0..4u32 {
            let mut seen = std::collections::HashSet::new();
            for y in 0..extent.height {
                for x in 0..extent.width {
                    let (logical_x, logical_y, _, _) = physical_to_logical(x, y, extent, turns);
                    assert!(seen.insert((logical_x, logical_y)), "повтор при turns={turns}");
                }
            }
            assert_eq!(seen.len(), (extent.width * extent.height) as usize);
        }
    }

    #[test]
    fn ten_bit_levels_reach_1023_and_blue_wraps_within_levels() {
        let levels = expected_levels(96, 60, EXTENT, 0, SurfaceTransform::Identity, ColorDepth::Unorm10).unwrap();
        assert_eq!(levels[0], 1023);
        assert_eq!(levels[1], 1023);
        for frame in 0..40u32 {
            let [_, _, blue] =
                expected_levels(10, 30, EXTENT, frame, SurfaceTransform::Identity, ColorDepth::Unorm10).unwrap();
            assert!(blue <= 1023);
        }
    }

    #[test]
    fn single_pixel_and_one_row_extents_do_not_divide_by_zero() {
        let tiny = vk::Extent2D { width: 1, height: 1 };
        assert_eq!(expected_pixel(0, 0, tiny, 1), [255, 255, 255, 255]);
        let line = vk::Extent2D { width: 5, height: 1 };
        // Высота 1: вся строка — полоса метки; x=4 попадает в клетку 4*8/5=6, бит 6 числа 3 равен 0.
        assert_eq!(expected_pixel(4, 0, line, 3), [0, 0, 0, 255]);
        // x=0 — клетка 0, бит 0 числа 3 равен 1.
        assert_eq!(expected_pixel(0, 0, line, 3), [255, 255, 255, 255]);
    }

    #[test]
    fn mirrored_transform_is_unsupported_and_srgb_format_is_rejected() {
        let error = expected_pixel_with_transform(0, 0, EXTENT, 0, SurfaceTransform::HorizontalMirror).unwrap_err();
        assert!(matches!(error, GpuError::Unsupported { .. }));
        assert!(matches!(
            ColorDepth::for_format(vk::Format::R8G8B8A8_SRGB),
            Err(GpuError::Unsupported { .. })
        ));
        assert_eq!(
            ColorDepth::for_format(vk::Format::B8G8R8A8_UNORM).unwrap(),
            ColorDepth::Unorm8
        );
        assert_eq!(
            ColorDepth::for_format(vk::Format::A2B10G10R10_UNORM_PACK32)
                .unwrap()
                .levels(),
            1023
        );
    }
}
