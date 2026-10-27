//! CPU-эталон предпросмотра RAW-мозаики: та же математика, что в `raw_mosaic_main.glsl`.
//!
//! Целочисленные шаги (повороты, вписывание, выбор ячейки) совпадают с шейдером точно; нормализация
//! идёт в `f32` и округляется «к ближайшему», как запись `UNORM8`, — расхождение с GPU не больше
//! 1 уровня младшего разряда (на этом допуске построены тесты паритета).

use super::params::{CfaChannel, MosaicPreviewParams, QuarterTurns};
use crate::offscreen::Rgba8Image;
use crate::test_pattern::SurfaceTransform;

/// Пиксели кадра для эталона.
#[derive(Debug, Clone, Copy)]
pub enum ReferenceRaw<'data> {
    /// RAW16: `width * height` значений, строки подряд.
    Raw16 {
        /// Ширина кадра, пиксели.
        width: u32,
        /// Значения.
        texels: &'data [u16],
    },
    /// RAW10: упакованные строки с шагом `row_stride_bytes`.
    Raw10 {
        /// Шаг строки, байт.
        row_stride_bytes: u32,
        /// Байты буфера.
        bytes: &'data [u8],
    },
}

impl ReferenceRaw<'_> {
    /// Значение пикселя кадра в абсолютных координатах буфера.
    fn pixel(&self, x: u32, y: u32) -> u32 {
        match *self {
            Self::Raw16 { width, texels } => u32::from(texels[y as usize * width as usize + x as usize]),
            Self::Raw10 {
                row_stride_bytes,
                bytes,
            } => unpack_raw10_pixel(bytes, row_stride_bytes, x, y),
        }
    }
}

/// Распаковывает один пиксель RAW10: группа из 4 пикселей занимает 5 байт,
/// `пиксель_i = (байт_i << 2) | ((байт_4 >> (2*i)) & 3)`.
pub fn unpack_raw10_pixel(bytes: &[u8], row_stride_bytes: u32, x: u32, y: u32) -> u32 {
    let group_offset = y as usize * row_stride_bytes as usize + (x as usize / 4) * 5;
    let index_in_group = (x % 4) as usize;
    let high = u32::from(bytes[group_offset + index_in_group]);
    let low_bits = (u32::from(bytes[group_offset + 4]) >> (2 * index_in_group)) & 3;
    (high << 2) | low_bits
}

/// Упаковывает 10-битные значения (по строкам, `width` кратна 4) в RAW10 с шагом `row_stride_bytes`.
/// Байты паддинга строки заполняются `padding_fill`: если шейдер их прочтёт, тест это заметит.
pub fn pack_raw10(values: &[u16], width: u32, row_stride_bytes: u32, padding_fill: u8) -> Vec<u8> {
    let rows = values.len() / width as usize;
    let mut bytes = vec![padding_fill; rows * row_stride_bytes as usize];
    for row in 0..rows {
        for group in 0..(width as usize / 4) {
            let offset = row * row_stride_bytes as usize + group * 5;
            let mut low_bits = 0u8;
            for index in 0..4 {
                let value = values[row * width as usize + group * 4 + index] & 0x3FF;
                bytes[offset + index] = (value >> 2) as u8;
                low_bits |= ((value & 3) as u8) << (2 * index);
            }
            bytes[offset + 4] = low_bits;
        }
    }
    bytes
}

/// Строит эталонную картинку `target_width × target_height`, как её нарисует шейдер при `display_transform`.
pub fn raw_mosaic_preview_reference(
    target_width: u32,
    target_height: u32,
    display_transform: SurfaceTransform,
    raw: ReferenceRaw<'_>,
    params: &MosaicPreviewParams,
) -> Rgba8Image {
    let display_turns = display_transform.quarter_turns().unwrap_or(0);
    let mut pixels = Vec::with_capacity(target_width as usize * target_height as usize * 4);
    for y in 0..target_height {
        for x in 0..target_width {
            let rgb = preview_pixel(x, y, (target_width, target_height), display_turns, raw, params);
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    Rgba8Image {
        width: target_width,
        height: target_height,
        pixels,
    }
}

/// Один пиксель цели (RGB, 8 бит).
fn preview_pixel(
    physical_x: u32,
    physical_y: u32,
    target: (u32, u32),
    display_turns: u32,
    raw: ReferenceRaw<'_>,
    params: &MosaicPreviewParams,
) -> [u8; 3] {
    const LETTERBOX: [u8; 3] = [0, 0, 0];
    let (target_width, target_height) = target;
    // 1. Физические координаты -> логические.
    let (logical_x, logical_y, logical_width, logical_height) = match display_turns {
        1 => (target_height - 1 - physical_y, physical_x, target_height, target_width),
        2 => (
            target_width - 1 - physical_x,
            target_height - 1 - physical_y,
            target_width,
            target_height,
        ),
        3 => (physical_y, target_width - 1 - physical_x, target_height, target_width),
        _ => (physical_x, physical_y, target_width, target_height),
    };
    // 2. Размер прямой картинки.
    let area = params.valid_area;
    let (upright_width, upright_height) = if params.sensor_rotation.swaps_axes() {
        (area.height, area.width)
    } else {
        (area.width, area.height)
    };
    // 3. Вписывание с сохранением пропорций.
    let (fitted_width, fitted_height) = if logical_width * upright_height <= logical_height * upright_width {
        (logical_width, (logical_width * upright_height / upright_width).max(1))
    } else {
        ((logical_height * upright_width / upright_height).max(1), logical_height)
    };
    let offset_x = (logical_width - fitted_width) / 2;
    let offset_y = (logical_height - fitted_height) / 2;
    if logical_x < offset_x
        || logical_y < offset_y
        || logical_x >= offset_x + fitted_width
        || logical_y >= offset_y + fitted_height
    {
        return LETTERBOX;
    }
    // 4. Координаты в прямой картинке.
    let upright_x = ((logical_x - offset_x) * upright_width / fitted_width).min(upright_width - 1);
    let upright_y = ((logical_y - offset_y) * upright_height / fitted_height).min(upright_height - 1);
    // 5. Обратный поворот в координаты сенсора.
    let (sensor_x, sensor_y) = match params.sensor_rotation {
        QuarterTurns::None => (upright_x, upright_y),
        QuarterTurns::Clockwise90 => (upright_y, area.height - 1 - upright_x),
        QuarterTurns::Clockwise180 => (area.width - 1 - upright_x, area.height - 1 - upright_y),
        QuarterTurns::Clockwise270 => (area.width - 1 - upright_y, upright_x),
    };
    // 6. Ячейка 2x2 и её значения.
    let cell_x = area.left + (sensor_x & !1);
    let cell_y = area.top + (sensor_y & !1);
    let values = [
        raw.pixel(cell_x, cell_y),
        raw.pixel(cell_x + 1, cell_y),
        raw.pixel(cell_x, cell_y + 1),
        raw.pixel(cell_x + 1, cell_y + 1),
    ];
    // 7. Нормализация и сборка RGB.
    let channels = params.cfa.channels();
    let (mut red, mut blue, mut green_sum) = (0.0f32, 0.0f32, 0.0f32);
    for position in 0..4 {
        let level = normalized_level(values[position], params.black_levels[position], params.white_level);
        match channels[position] {
            CfaChannel::Red => red = level,
            CfaChannel::Blue => blue = level,
            CfaChannel::Green => green_sum += level,
        }
    }
    [to_unorm8(red), to_unorm8(green_sum * 0.5), to_unorm8(blue)]
}

fn normalized_level(value: u32, black: u32, white: u32) -> f32 {
    let numerator = value as i64 - i64::from(black);
    let denominator = (i64::from(white) - i64::from(black)).max(1);
    (numerator as f32 / denominator as f32).clamp(0.0, 1.0)
}

/// Запись `UNORM8`: округление к ближайшему.
fn to_unorm8(level: f32) -> u8 {
    (level * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw_mosaic_preview::params::{CfaOrder, ValidArea};

    #[test]
    fn raw10_pack_and_unpack_round_trip_every_value_and_ignore_padding() {
        let width = 8;
        let values: Vec<u16> = (0..width * 3).map(|index| ((index * 37 + 5) % 1024) as u16).collect();
        let stride = 16; // 10 полезных байт + 6 паддинга
        let bytes = pack_raw10(&values, width, stride, 0xEE);
        for (index, expected) in values.iter().enumerate() {
            let (x, y) = ((index as u32) % width, (index as u32) / width);
            assert_eq!(
                unpack_raw10_pixel(&bytes, stride, x, y),
                u32::from(*expected),
                "({x},{y})"
            );
        }
        assert_eq!(bytes[10], 0xEE, "паддинг строки не тронут упаковкой");
    }

    #[test]
    fn black_is_zero_white_is_full_and_below_black_is_clamped() {
        let params = MosaicPreviewParams {
            valid_area: ValidArea {
                left: 0,
                top: 0,
                width: 2,
                height: 2,
            },
            cfa: CfaOrder::Rggb,
            black_levels: [64; 4],
            white_level: 1023,
            sensor_rotation: QuarterTurns::None,
        };
        // R = белый, G1 = чёрный, G2 = ниже чёрного, B = середина диапазона.
        let texels = [1023u16, 64, 10, 544];
        let image = raw_mosaic_preview_reference(
            2,
            2,
            SurfaceTransform::Identity,
            ReferenceRaw::Raw16 {
                width: 2,
                texels: &texels,
            },
            &params,
        );
        assert_eq!(image.pixel(0, 0), Some([255, 0, 128, 255]));
    }
}
