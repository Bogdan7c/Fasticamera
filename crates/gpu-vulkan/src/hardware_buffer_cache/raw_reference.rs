//! Детерминированный образец RAW-кадра и CPU-эталон контрольных сумм.
//!
//! Это «правда», с которой сравнивается результат compute-шейдеров `ahb_sum_r16.comp` и
//! `ahb_sum_words.comp`. Алгоритмы обязаны совпадать с шейдерами до бита:
//! * `sum` — сумма значений по модулю 2^32;
//! * `mix` — XOR членов `rotl(значение, индекс & 31) ^ (индекс * 0x9E3779B1)` (все операции по модулю 2^32).
//!
//! 64-битных атомиков на Adreno нет (зонд 00), поэтому суммы 32-битные: для проверки «GPU видит те же
//! байты, что и CPU» этого достаточно, а `mix` ловит перестановки, которых сумма не замечает.
//! Паддинг строки в образце забит сентинелом [`PADDING_SENTINEL`]: если шейдер или шаг строки
//! ошибочно захватят паддинг, суммы разойдутся.

use std::fmt;

/// Байт, которым забит паддинг строки в образце.
pub const PADDING_SENTINEL: u8 = 0xA5;
/// Множитель позиционного смешивания (то же число в шейдерах).
const MIX_MULTIPLIER: u32 = 0x9E37_79B1;

/// Формат RAW-кадра, который умеет диагностика.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RawPixelFormat {
    /// 16 бит на пиксель (AHB `0x20`), импортируется картинкой `R16_UINT`.
    Raw16,
    /// Упакованный 10 бит на пиксель (AHB `0x25`, 4 пикселя в 5 байтах), импортируется буфером.
    Raw10,
}

impl RawPixelFormat {
    /// Код формата `AHardwareBuffer_Desc::format` (`HAL_PIXEL_FORMAT_RAW16`/`RAW10`; в публичном
    /// `hardware_buffer.h` NDK 30 именованных констант для них нет — значения из факта 00, раздел C4).
    pub const fn ahb_format_code(self) -> u32 {
        match self {
            RawPixelFormat::Raw16 => 0x20,
            RawPixelFormat::Raw10 => 0x25,
        }
    }

    /// Байт реальных данных в строке шириной `width` пикселей (без паддинга).
    pub fn valid_row_bytes(self, width: u32) -> u64 {
        match self {
            RawPixelFormat::Raw16 => u64::from(width) * 2,
            // 10 бит на пиксель, округление вниз до целого байта.
            RawPixelFormat::Raw10 => u64::from(width) * 10 / 8,
        }
    }
}

/// Геометрия строк кадра в байтах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowGeometry {
    /// Число строк.
    pub rows: u32,
    /// Полезных байт в строке.
    pub row_valid_bytes: u64,
    /// Шаг между началами строк, байт.
    pub row_stride_bytes: u64,
}

impl RowGeometry {
    /// Сколько байт от начала буфера покрывают все полезные данные (у последней строки паддинга нет).
    pub fn required_bytes(&self) -> u64 {
        match self.rows {
            0 => 0,
            rows => u64::from(rows - 1) * self.row_stride_bytes + self.row_valid_bytes,
        }
    }
}

/// Ошибка вывода шага строки из `AHardwareBuffer_describe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrideError {
    /// Ни одна трактовка `stride` не согласуется с размером аллокации — угадывать нельзя.
    Inconsistent {
        /// `stride` из `AHardwareBuffer_describe`.
        ahb_stride: u32,
        /// `allocationSize` из Vulkan.
        allocation_size: u64,
    },
}

impl fmt::Display for StrideError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let StrideError::Inconsistent {
            ahb_stride,
            allocation_size,
        } = self;
        write!(
            formatter,
            "stride AHB {ahb_stride} не согласуется с аллокацией {allocation_size} байт"
        )
    }
}

impl std::error::Error for StrideError {}

/// Выводит шаг строки в байтах по `stride` из `AHardwareBuffer_describe`.
///
/// * RAW16: `stride` в пикселях → `stride * 2` байт;
/// * RAW10: единицы `stride` у AHB не стандартизированы. Gralloc этого телефона для 4096×3072
///   отдаёт 5120 — это БАЙТЫ строки (4096·10/8), а не пиксели (факт 00, дефект 5a; аллокация ровно
///   5120×3072). Поэтому проверяем обе трактовки и берём ту, что вмещает пиксели и все строки в
///   `allocation_size`; если годятся обе, берём «байты» (их подтвердил телефон).
pub fn resolve_row_stride_bytes(
    format: RawPixelFormat,
    width: u32,
    height: u32,
    ahb_stride: u32,
    allocation_size: u64,
) -> Result<u64, StrideError> {
    let stride = u64::from(ahb_stride);
    match format {
        RawPixelFormat::Raw16 => Ok(stride * 2),
        RawPixelFormat::Raw10 => {
            let as_bytes = stride;
            let as_pixels = (stride * 10).div_ceil(8);
            let minimum_row = format.valid_row_bytes(width);
            [as_bytes, as_pixels]
                .into_iter()
                .find(|row_bytes| *row_bytes >= minimum_row && row_bytes * u64::from(height) <= allocation_size)
                .ok_or(StrideError::Inconsistent {
                    ahb_stride,
                    allocation_size,
                })
        }
    }
}

/// Пара контрольных сумм кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecksumPair {
    /// Сумма значений по модулю 2^32.
    pub sum: u32,
    /// XOR позиционно смешанных значений.
    pub mix: u32,
}

impl ChecksumPair {
    /// Учитывает одно значение, стоящее на логической позиции `index`.
    fn accumulate(&mut self, value: u32, index: u32) {
        self.sum = self.sum.wrapping_add(value);
        // `rotate_left` у Rust берёт amount по модулю 32 — как `& 31` в шейдере.
        self.mix ^= value.rotate_left(index & 31) ^ index.wrapping_mul(MIX_MULTIPLIER);
    }
}

/// Значение текселя RAW16 образца в точке `(column, row)`.
pub fn raw16_pattern_value(column: u32, row: u32) -> u16 {
    // Два разных простых множителя + XOR: значения занимают все 16 бит и не повторяются по строкам.
    (column.wrapping_mul(40503) ^ row.wrapping_mul(2_654_435_761).rotate_left(7)) as u16
}

/// Байт образца RAW10 на позиции `byte_in_row` строки `row`.
pub fn raw10_pattern_byte(byte_in_row: u64, row: u32) -> u8 {
    // Признак позиции, не совпадающий с паддингом хотя бы по части значений.
    ((byte_in_row.wrapping_mul(131) as u32) ^ row.wrapping_mul(97) ^ 0x3C) as u8
}

/// Заполняет `bytes` образцом: полезная часть строк — по формулам, паддинг — сентинел.
///
/// `bytes` должен вмещать `geometry.required_bytes()`; байты за пределами этого диапазона не
/// трогаются. Возвращает `false`, если срез короче — вызывающий обязан показать это как ошибку.
#[must_use = "false означает, что срез короче геометрии и образец не записан"]
pub fn fill_pattern(bytes: &mut [u8], format: RawPixelFormat, geometry: &RowGeometry) -> bool {
    let Ok(required) = usize::try_from(geometry.required_bytes()) else {
        return false;
    };
    if bytes.len() < required {
        return false;
    }
    for row in 0..geometry.rows {
        let row_start = row as usize * geometry.row_stride_bytes as usize;
        let row_end = (row_start + geometry.row_stride_bytes as usize).min(bytes.len());
        let valid_end = row_start + geometry.row_valid_bytes as usize;
        for (offset, slot) in bytes[row_start..row_end].iter_mut().enumerate() {
            let in_valid_part = row_start + offset < valid_end;
            *slot = if in_valid_part {
                match format {
                    RawPixelFormat::Raw16 => {
                        let column = (offset / 2) as u32;
                        raw16_pattern_value(column, row).to_le_bytes()[offset % 2]
                    }
                    RawPixelFormat::Raw10 => raw10_pattern_byte(offset as u64, row),
                }
            } else {
                PADDING_SENTINEL
            };
        }
    }
    true
}

/// Ошибка CPU-эталона: байт не хватает для заявленной геометрии.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferTooSmall {
    /// Нужно байт.
    pub required: u64,
    /// Есть байт.
    pub actual: u64,
}

impl fmt::Display for BufferTooSmall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "буфер {} байт, геометрии нужно {}",
            self.actual, self.required
        )
    }
}

impl std::error::Error for BufferTooSmall {}

fn require_bytes(bytes: &[u8], geometry: &RowGeometry) -> Result<(), BufferTooSmall> {
    let required = geometry.required_bytes();
    let actual = bytes.len() as u64;
    if actual < required {
        return Err(BufferTooSmall { required, actual });
    }
    Ok(())
}

/// Эталон для пути «картинка R16_UINT»: тексели `u16` little-endian, `width` штук в строке.
pub fn checksum_u16_texels(bytes: &[u8], width: u32, geometry: &RowGeometry) -> Result<ChecksumPair, BufferTooSmall> {
    require_bytes(bytes, geometry)?;
    let mut checksum = ChecksumPair { sum: 0, mix: 0 };
    for row in 0..geometry.rows {
        let row_start = row as usize * geometry.row_stride_bytes as usize;
        for column in 0..width {
            let offset = row_start + column as usize * 2;
            let value = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            checksum.accumulate(u32::from(value), row.wrapping_mul(width).wrapping_add(column));
        }
    }
    Ok(checksum)
}

/// Эталон для пути «буфер»: 32-битные слова little-endian в первых `row_valid_bytes / 4` словах
/// каждой строки (хвост 1–3 байта строки не учитывается — шейдер работает целыми словами).
pub fn checksum_strided_words(bytes: &[u8], geometry: &RowGeometry) -> Result<ChecksumPair, BufferTooSmall> {
    require_bytes(bytes, geometry)?;
    let words_per_row = (geometry.row_valid_bytes / 4) as u32;
    let mut checksum = ChecksumPair { sum: 0, mix: 0 };
    for row in 0..geometry.rows {
        let row_start = row as usize * geometry.row_stride_bytes as usize;
        for column in 0..words_per_row {
            let offset = row_start + column as usize * 4;
            let value = u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);
            checksum.accumulate(value, row.wrapping_mul(words_per_row).wrapping_add(column));
        }
    }
    Ok(checksum)
}

#[cfg(test)]
mod tests;
