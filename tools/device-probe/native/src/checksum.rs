//! Эталонные CPU-контрольные суммы кадра с учётом stride.
//!
//! Это «правда», с которой сравнивается результат compute-шейдера. Алгоритм обязан
//! совпадать с шейдерами в `shaders/`: сумма **по модулю 2^32** (`u32` wraparound).
//! 64-битные атомики на Adreno/Android доступны не везде, поэтому берём u32 —
//! для проверки «GPU видит те же байты, что и CPU» этого достаточно (коллизия
//! при случайных данных ~ 2^-32), а переполнение — ожидаемое и одинаковое на обеих сторонах.
//!
//! Модуль чистый: принимает срез байт (на устройстве — замапленный `AHardwareBuffer_lock`).

use thiserror::Error;

use crate::camera_metadata::{FORMAT_RAW10, FORMAT_RAW16};

/// Ошибки разбора геометрии буфера. Каждая — признак неверных входных параметров,
/// а не «сумма не сошлась».
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChecksumError {
    /// Строка (stride) короче, чем полезная часть строки.
    #[error("stride {row_stride_bytes} байт меньше полезной строки {row_valid_bytes} байт")]
    StrideSmallerThanRow {
        row_stride_bytes: usize,
        row_valid_bytes: usize,
    },
    /// Срез байт не покрывает заявленную геометрию.
    #[error("буфер {actual} байт, а геометрия требует {required}")]
    BufferTooSmall { required: usize, actual: usize },
}

/// Сколько байт в строке реально занято пикселями (без паддинга до stride).
///
/// * RAW16 — 2 байта на пиксель;
/// * RAW10 — упакованный формат, 10 бит на пиксель (4 пикселя в 5 байтах), округление вниз;
/// * остальное (RAW_PRIVATE и т.п.) — внутреннее устройство не определено NDK,
///   поэтому считаем полезной всю строку до stride.
pub fn valid_row_bytes(format: i32, width: usize, row_stride_bytes: usize) -> usize {
    match format {
        FORMAT_RAW16 => width * 2,
        FORMAT_RAW10 => width * 10 / 8,
        _ => row_stride_bytes,
    }
}

/// Геометрия строк кадра в байтах.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct RowLayout {
    pub rows: usize,
    /// Байт реальных данных в строке.
    pub row_valid_bytes: usize,
    /// Шаг между началами строк.
    pub row_stride_bytes: usize,
}

impl RowLayout {
    /// Сколько байт от начала буфера нужно, чтобы покрыть все строки (у последней нет паддинга).
    pub fn required_bytes(&self) -> usize {
        match self.rows {
            0 => 0,
            rows => (rows - 1) * self.row_stride_bytes + self.row_valid_bytes,
        }
    }
}

/// Stride строки по данным `AHardwareBuffer_describe`, если `AImage` его не дал.
/// `None` — для формата вычислить нельзя (например RAW_PRIVATE).
///
/// Единицы `stride` у AHB для упакованного RAW10 не стандартизированы: gralloc этого телефона
/// отдаёт для RAW10 4096×3072 `stride = 5120` — это **байты** строки (4096 × 10 / 8), а не пиксели
/// (факт сессии 00: аллокация ровно 5120 × 3072). Поэтому для RAW10 проверяем обе трактовки и берём ту,
/// что согласуется с размером аллокации; без размера аллокации трактовку выбрать нельзя — это ошибка.
fn fallback_row_stride_bytes(
    format: i32,
    ahb_stride: u32,
    width: usize,
    height: usize,
    allocation_bytes: Option<usize>,
) -> Option<usize> {
    let stride = ahb_stride as usize;
    match format {
        FORMAT_RAW16 => Some(stride * 2),
        FORMAT_RAW10 => {
            let allocation = allocation_bytes?;
            let as_bytes = stride;
            let as_pixels = (stride * 10).div_ceil(8);
            let min_row_bytes = valid_row_bytes(FORMAT_RAW10, width, 0);
            // Трактовка годится, если строка вмещает пиксели и все строки помещаются в аллокацию.
            // Если годятся обе, берём «байты»: именно её подтвердил этот телефон.
            [as_bytes, as_pixels]
                .into_iter()
                .find(|row_bytes| *row_bytes >= min_row_bytes && row_bytes * height <= allocation)
        }
        _ => None,
    }
}

/// Определяет геометрию строк: приоритет у `AImage_getPlaneRowStride`, иначе вывод из AHB.
pub fn row_layout(
    format: i32,
    width: usize,
    height: usize,
    plane0_row_stride: Option<i32>,
    ahb_stride: u32,
    allocation_bytes: Option<usize>,
) -> Result<RowLayout, String> {
    let row_stride_bytes = plane0_row_stride
        .and_then(|stride| usize::try_from(stride).ok())
        .filter(|stride| *stride > 0)
        .or_else(|| fallback_row_stride_bytes(format, ahb_stride, width, height, allocation_bytes))
        .ok_or_else(|| {
            format!("stride строки неизвестен для формата 0x{format:X}: AImage его не отдал, вывести из AHB нельзя")
        })?;
    let row_valid_bytes = valid_row_bytes(format, width, row_stride_bytes);
    if row_stride_bytes < row_valid_bytes {
        return Err(format!(
            "stride {row_stride_bytes} меньше полезной строки {row_valid_bytes}"
        ));
    }
    Ok(RowLayout {
        rows: height,
        row_valid_bytes,
        row_stride_bytes,
    })
}

/// Проверяет, что `bytes_len` вмещает `rows` строк с шагом `row_stride_bytes`
/// (последней строке паддинг не нужен).
fn require_geometry(
    bytes_len: usize,
    rows: usize,
    row_stride_bytes: usize,
    row_valid_bytes: usize,
) -> Result<(), ChecksumError> {
    if row_stride_bytes < row_valid_bytes {
        return Err(ChecksumError::StrideSmallerThanRow {
            row_stride_bytes,
            row_valid_bytes,
        });
    }
    let required = match rows {
        0 => 0,
        _ => (rows - 1) * row_stride_bytes + row_valid_bytes,
    };
    if bytes_len < required {
        return Err(ChecksumError::BufferTooSmall {
            required,
            actual: bytes_len,
        });
    }
    Ok(())
}

/// Сумма 16-битных little-endian текселей изображения `width × height`
/// (путь «VkImage R16_UINT»). Паддинг строки (stride > width*2) не учитывается.
pub fn sum_u16_texels(
    bytes: &[u8],
    width: usize,
    height: usize,
    row_stride_bytes: usize,
) -> Result<u32, ChecksumError> {
    let row_valid_bytes = width * 2;
    require_geometry(bytes.len(), height, row_stride_bytes, row_valid_bytes)?;
    let mut sum = 0_u32;
    for row in 0..height {
        let row_start = row * row_stride_bytes;
        for texel in bytes[row_start..row_start + row_valid_bytes].chunks_exact(2) {
            sum = sum.wrapping_add(u32::from(u16::from_le_bytes([texel[0], texel[1]])));
        }
    }
    Ok(sum)
}

/// Сумма 32-битных little-endian слов, лежащих в первых `row_valid_bytes` каждой строки
/// (путь «VkBuffer»; подходит и для упакованного RAW10).
///
/// Если `row_valid_bytes` не кратно 4, хвост строки (1–3 байта) **не** суммируется —
/// шейдер `sum_words.comp` работает целыми словами и делает ровно то же.
pub fn sum_strided_words(
    bytes: &[u8],
    rows: usize,
    row_valid_bytes: usize,
    row_stride_bytes: usize,
) -> Result<u32, ChecksumError> {
    require_geometry(bytes.len(), rows, row_stride_bytes, row_valid_bytes)?;
    let words_per_row = row_valid_bytes / 4;
    let mut sum = 0_u32;
    for row in 0..rows {
        let row_start = row * row_stride_bytes;
        let row_words = bytes[row_start..row_start + words_per_row * 4].chunks_exact(4);
        for word in row_words {
            sum = sum.wrapping_add(u32::from_le_bytes([word[0], word[1], word[2], word[3]]));
        }
    }
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Строка из `texels` (u16 LE) + `padding` байт мусора 0xFF.
    fn row_with_padding(texels: &[u16], padding: usize) -> Vec<u8> {
        let mut row: Vec<u8> = texels.iter().flat_map(|t| t.to_le_bytes()).collect();
        row.extend(std::iter::repeat_n(0xFF, padding));
        row
    }

    #[test]
    fn u16_sum_ignores_row_padding() {
        // 3x2 тексела, stride 10 байт = 6 полезных + 4 мусорных 0xFF.
        let mut bytes = row_with_padding(&[1, 2, 3], 4);
        bytes.extend(row_with_padding(&[10, 20, 30], 4));
        assert_eq!(sum_u16_texels(&bytes, 3, 2, 10), Ok(66));
    }

    #[test]
    fn u16_sum_last_row_may_omit_padding() {
        let mut bytes = row_with_padding(&[100, 200], 4);
        bytes.extend(row_with_padding(&[300, 400], 0)); // у последней строки нет хвоста
        assert_eq!(sum_u16_texels(&bytes, 2, 2, 8), Ok(1000));
    }

    #[test]
    fn u16_sum_wraps_modulo_2_pow_32() {
        // 0x1_0002 текселей по 0xFFFF → сумма вылезает за u32 и должна завернуться.
        let count = 0x1_0002_usize;
        let bytes: Vec<u8> = std::iter::repeat_n([0xFF_u8, 0xFF], count).flatten().collect();
        let expected = (0xFFFF_u64 * count as u64 % (1_u64 << 32)) as u32;
        assert_eq!(sum_u16_texels(&bytes, count, 1, count * 2), Ok(expected));
        assert!(0xFFFF_u64 * count as u64 > u64::from(u32::MAX));
    }

    #[test]
    fn u16_sum_rejects_short_buffer_and_small_stride() {
        assert_eq!(
            sum_u16_texels(&[0; 5], 3, 1, 6),
            Err(ChecksumError::BufferTooSmall { required: 6, actual: 5 })
        );
        assert_eq!(
            sum_u16_texels(&[0; 64], 4, 2, 6),
            Err(ChecksumError::StrideSmallerThanRow {
                row_stride_bytes: 6,
                row_valid_bytes: 8
            })
        );
    }

    #[test]
    fn words_sum_respects_stride_and_drops_partial_tail_word() {
        // 2 строки, stride 12, полезных 10 байт → 2 целых слова на строку, 2 байта хвоста пропускаются.
        let mut bytes = Vec::new();
        for (a, b) in [(1_u32, 2_u32), (30, 40)] {
            bytes.extend(a.to_le_bytes());
            bytes.extend(b.to_le_bytes());
            bytes.extend([0xEE, 0xEE, 0xEE, 0xEE]); // хвост + паддинг, не должен попасть в сумму
        }
        assert_eq!(sum_strided_words(&bytes, 2, 10, 12), Ok(73));
    }

    #[test]
    fn words_sum_packed_raw10_row() {
        // RAW10, ширина 8 → 10 полезных байт; stride 16.
        assert_eq!(valid_row_bytes(FORMAT_RAW10, 8, 16), 10);
        let mut bytes = vec![0_u8; 16 * 2];
        bytes[0] = 5; // слово 0 строки 0 = 5
        bytes[16] = 7; // слово 0 строки 1 = 7
        bytes[12] = 9; // лежит в паддинге строки 0 — не считается
        assert_eq!(sum_strided_words(&bytes, 2, 10, 16), Ok(12));
    }

    #[test]
    fn valid_row_bytes_per_format() {
        assert_eq!(valid_row_bytes(FORMAT_RAW16, 4096, 8320), 8192);
        assert_eq!(valid_row_bytes(FORMAT_RAW10, 4096, 5120), 5120);
        assert_eq!(valid_row_bytes(0x24, 4096, 6000), 6000);
    }

    #[test]
    fn zero_rows_is_zero_sum() {
        assert_eq!(sum_strided_words(&[], 0, 8, 8), Ok(0));
        assert_eq!(sum_u16_texels(&[], 4, 0, 8), Ok(0));
    }

    #[test]
    fn row_layout_prefers_plane_stride() {
        let layout = row_layout(FORMAT_RAW16, 4096, 3, Some(8448), 4096, None).unwrap();
        assert_eq!(
            layout,
            RowLayout {
                rows: 3,
                row_valid_bytes: 8192,
                row_stride_bytes: 8448
            }
        );
        assert_eq!(layout.required_bytes(), 2 * 8448 + 8192);
    }

    #[test]
    fn row_layout_falls_back_to_ahb_stride() {
        let raw16 = row_layout(FORMAT_RAW16, 100, 2, None, 128, None).unwrap();
        assert_eq!(raw16.row_stride_bytes, 256);
        // stride=100 как «байты» не вмещает 100 пикселей RAW10 (нужно 125) → это пиксели: 125 байт.
        let raw10 = row_layout(FORMAT_RAW10, 100, 2, None, 100, Some(250)).unwrap();
        assert_eq!((raw10.row_valid_bytes, raw10.row_stride_bytes), (125, 125));
    }

    #[test]
    fn raw10_stride_in_bytes_matches_phone_allocation() {
        // Реальные числа OnePlus 11: RAW10 4096×3072, AHB stride 5120, аллокация 15 728 640.
        let layout = row_layout(FORMAT_RAW10, 4096, 3072, None, 5120, Some(15_728_640)).unwrap();
        assert_eq!((layout.row_valid_bytes, layout.row_stride_bytes), (5120, 5120));
        assert_eq!(layout.required_bytes(), 15_728_640);
    }

    #[test]
    fn raw10_without_allocation_size_is_an_error_not_a_guess() {
        assert!(row_layout(FORMAT_RAW10, 4096, 3072, None, 5120, None).is_err());
    }

    #[test]
    fn row_layout_rejects_unknown_stride_and_too_small_stride() {
        assert!(row_layout(0x24, 64, 64, None, 64, Some(1 << 20)).is_err());
        assert!(row_layout(FORMAT_RAW16, 64, 4, Some(100), 64, None).is_err());
    }
}
