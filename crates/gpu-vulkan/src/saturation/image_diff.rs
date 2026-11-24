//! Сравнение двух картинок по байтам: насколько сильно они различаются (нейтральный помощник стенда насыщения).
//!
//! # Зачем (простыми словами)
//! Отпечаток выхода (`OutputDigest`) отвечает только «совпало бит в бит или нет». Для неточных оптимизаций
//! (решение владельца 213: допустимо отличие не больше 1 младшего разряда 10-битного выхода) нужен ответ
//! «насколько именно различается»: наибольшая разница по каналам, сколько пикселей различаются хотя бы на 1 и
//! на 2 разряда, и как разница распределена. Помощник не знает ни Vulkan, ни конкретных работ: ему дают два
//! буфера байтов с плотными строками (без отступов) и описание формата пикселя.
//!
//! # Единицы разницы
//! Разница считается в «кодах» самого формата, но для плавающего формата приводится к тому же масштабу:
//! * [`ImageFormat::Rgb10A2`] — коды 0…1023 цветовых каналов (альфа — коды 0…3, в счёт пикселей не входит);
//! * [`ImageFormat::Rgba8`] — коды 0…255 (шаг 8-битного выхода крупнее 10-битного вчетверо; отчёт честно
//!   говорит в своих единицах, поэтому формат записан в отчёте);
//! * [`ImageFormat::Rgba16F`] — значение канала переводится в код 10-битного выхода `round(v · 1023)`, обрезанный
//!   в 0…1023; разница — между такими кодами. Альфа (всегда 1) в счёт пикселей не входит.
//!
//! Пиксель «различается на N», если наибольшая разница по цветовым каналам R, G, B не меньше N.

use std::fmt;

/// Формат пикселя сравниваемых буферов (порядок байтов — как в памяти Vulkan для little-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// `A2B10G10R10_UNORM_PACK32`: одно слово u32 на пиксель, биты 0–9 — R, 10–19 — G, 20–29 — B, 30–31 — A.
    Rgb10A2,
    /// `R16G16B16A16_SFLOAT`: четыре half-float подряд (R, G, B, A), по 2 байта.
    Rgba16F,
    /// `R8G8B8A8_UNORM`: четыре байта подряд (R, G, B, A).
    Rgba8,
}

impl ImageFormat {
    /// Сколько байт занимает один пиксель.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb10A2 | Self::Rgba8 => 4,
            Self::Rgba16F => 8,
        }
    }

    /// Имя формата для отчёта.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rgb10A2 => "rgb10a2",
            Self::Rgba16F => "rgba16f",
            Self::Rgba8 => "rgba8",
        }
    }
}

/// Число корзин гистограммы: корзина `i < 7` — пиксели с наибольшей разницей ровно `i`, последняя — `≥ 7`.
pub const HISTOGRAM_BINS: usize = 8;

/// Почему сравнить не вышло.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageDiffError {
    /// Буферы разной длины: это картинки разного размера или формата.
    #[error("буферы разной длины: {first} и {second} байт")]
    LengthMismatch {
        /// Длина первого буфера.
        first: usize,
        /// Длина второго буфера.
        second: usize,
    },
    /// Длина не кратна размеру пикселя формата: строки с отступом или не тот формат.
    #[error("длина {length} байт не кратна размеру пикселя {bytes_per_pixel} формата {format}")]
    NotWholePixels {
        /// Длина буфера.
        length: usize,
        /// Размер пикселя формата.
        bytes_per_pixel: usize,
        /// Имя формата.
        format: &'static str,
    },
}

/// Итог сравнения двух картинок.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageDiffReport {
    /// Формат, в кодах которого записаны разницы.
    pub format: ImageFormat,
    /// Сколько пикселей сравнено.
    pub pixel_count: u64,
    /// Наибольшая разница по каналам R, G, B (в кодах формата, см. описание модуля).
    pub max_delta: [u32; 3],
    /// Наибольшая разница по альфе (в кодах канала альфа; в счёт пикселей не входит).
    pub max_alpha_delta: u32,
    /// Пикселей, различающихся хотя бы на 1 код.
    pub differing_at_least_1: u64,
    /// Пикселей, различающихся хотя бы на 2 кода.
    pub differing_at_least_2: u64,
    /// Гистограмма: сколько пикселей с наибольшей разницей `i` (последняя корзина — `≥ HISTOGRAM_BINS − 1`).
    pub histogram: [u64; HISTOGRAM_BINS],
}

impl ImageDiffReport {
    /// Наибольшая разница по всем цветовым каналам.
    pub fn max_color_delta(&self) -> u32 {
        self.max_delta.into_iter().max().unwrap_or(0)
    }

    /// Картинки совпали полностью (цвет и альфа).
    pub fn is_identical(&self) -> bool {
        self.max_color_delta() == 0 && self.max_alpha_delta == 0
    }
}

impl fmt::Display for ImageDiffReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: пикселей {}, max|Δ| R/G/B = {}/{}/{}, Δ≥1: {}, Δ≥2: {}, гистограмма {:?}",
            self.format.name(),
            self.pixel_count,
            self.max_delta[0],
            self.max_delta[1],
            self.max_delta[2],
            self.differing_at_least_1,
            self.differing_at_least_2,
            self.histogram
        )
    }
}

/// Сравнивает два плотных буфера пикселей `format`.
pub fn compare_image_bytes(
    first: &[u8],
    second: &[u8],
    format: ImageFormat,
) -> Result<ImageDiffReport, ImageDiffError> {
    if first.len() != second.len() {
        return Err(ImageDiffError::LengthMismatch {
            first: first.len(),
            second: second.len(),
        });
    }
    let bytes_per_pixel = format.bytes_per_pixel();
    if !first.len().is_multiple_of(bytes_per_pixel) {
        return Err(ImageDiffError::NotWholePixels {
            length: first.len(),
            bytes_per_pixel,
            format: format.name(),
        });
    }
    let mut report = ImageDiffReport {
        format,
        pixel_count: (first.len() / bytes_per_pixel) as u64,
        max_delta: [0; 3],
        max_alpha_delta: 0,
        differing_at_least_1: 0,
        differing_at_least_2: 0,
        histogram: [0; HISTOGRAM_BINS],
    };
    for (pixel_first, pixel_second) in first
        .chunks_exact(bytes_per_pixel)
        .zip(second.chunks_exact(bytes_per_pixel))
    {
        let (colors_first, alpha_first) = decode_pixel(pixel_first, format);
        let (colors_second, alpha_second) = decode_pixel(pixel_second, format);
        let mut pixel_max = 0;
        for channel in 0..3 {
            let delta = colors_first[channel].abs_diff(colors_second[channel]);
            report.max_delta[channel] = report.max_delta[channel].max(delta);
            pixel_max = pixel_max.max(delta);
        }
        report.max_alpha_delta = report.max_alpha_delta.max(alpha_first.abs_diff(alpha_second));
        report.differing_at_least_1 += u64::from(pixel_max >= 1);
        report.differing_at_least_2 += u64::from(pixel_max >= 2);
        report.histogram[(pixel_max as usize).min(HISTOGRAM_BINS - 1)] += 1;
    }
    Ok(report)
}

/// Коды цветовых каналов R, G, B и код альфы одного пикселя (единицы — в описании модуля).
fn decode_pixel(pixel: &[u8], format: ImageFormat) -> ([u32; 3], u32) {
    match format {
        ImageFormat::Rgb10A2 => {
            let word = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
            ([word & 0x3FF, (word >> 10) & 0x3FF, (word >> 20) & 0x3FF], word >> 30)
        }
        ImageFormat::Rgba8 => ([pixel[0].into(), pixel[1].into(), pixel[2].into()], pixel[3].into()),
        ImageFormat::Rgba16F => {
            let channel =
                |index: usize| half_to_ten_bit_code(u16::from_le_bytes([pixel[2 * index], pixel[2 * index + 1]]));
            ([channel(0), channel(1), channel(2)], channel(3))
        }
    }
}

/// Значение half-float в код 10-битного выхода: `round(v · 1023)`, обрезанный в 0…1023 (NaN → 0).
fn half_to_ten_bit_code(bits: u16) -> u32 {
    let value = half_bits_to_f32(bits);
    if value.is_nan() {
        return 0;
    }
    (value * 1023.0).round().clamp(0.0, 1023.0) as u32
}

/// IEEE 754 binary16 → f32 (точное преобразование, включая денормали и бесконечности).
fn half_bits_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0_f32 } else { 1.0_f32 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let fraction = f32::from(bits & 0x3FF);
    match exponent {
        0 => sign * fraction * 2.0_f32.powi(-24),
        0x1F if bits & 0x3FF == 0 => sign * f32::INFINITY,
        0x1F => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent - 15),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пиксель rgb10a2 из кодов R, G, B и альфы.
    fn rgb10a2(red: u32, green: u32, blue: u32, alpha: u32) -> [u8; 4] {
        (red | (green << 10) | (blue << 20) | (alpha << 30)).to_le_bytes()
    }

    #[test]
    fn identical_images_report_zero_difference() {
        let image: Vec<u8> = [rgb10a2(5, 6, 7, 3), rgb10a2(1000, 0, 512, 3)].concat();
        let report = compare_image_bytes(&image, &image, ImageFormat::Rgb10A2).unwrap();
        assert!(report.is_identical());
        assert_eq!(report.pixel_count, 2);
        assert_eq!(report.differing_at_least_1, 0);
        assert_eq!(report.histogram[0], 2);
    }

    #[test]
    fn rgb10a2_difference_is_counted_per_channel_and_per_pixel() {
        let first = [
            rgb10a2(100, 200, 300, 3),
            rgb10a2(0, 0, 0, 3),
            rgb10a2(1023, 1023, 1023, 3),
        ]
        .concat();
        // Первый пиксель: R на 1, B на 2 → наибольшая 2. Второй: G на 1. Третий: без изменений.
        let second = [
            rgb10a2(101, 200, 298, 3),
            rgb10a2(0, 1, 0, 3),
            rgb10a2(1023, 1023, 1023, 3),
        ]
        .concat();
        let report = compare_image_bytes(&first, &second, ImageFormat::Rgb10A2).unwrap();
        assert_eq!(report.max_delta, [1, 1, 2]);
        assert_eq!(report.differing_at_least_1, 2);
        assert_eq!(report.differing_at_least_2, 1);
        assert_eq!(
            report.histogram[..3],
            [1, 1, 1],
            "по пикселю с наибольшей разницей 0, 1 и 2"
        );
        assert!(!report.is_identical());
    }

    #[test]
    fn alpha_difference_is_reported_but_not_counted_as_a_differing_pixel() {
        let report = compare_image_bytes(&rgb10a2(9, 9, 9, 3), &rgb10a2(9, 9, 9, 1), ImageFormat::Rgb10A2).unwrap();
        assert_eq!(report.max_alpha_delta, 2);
        assert_eq!(report.differing_at_least_1, 0);
        assert!(!report.is_identical(), "альфа различается — картинки не идентичны");
    }

    #[test]
    fn large_differences_land_in_the_last_histogram_bin() {
        let report = compare_image_bytes(&rgb10a2(0, 0, 0, 3), &rgb10a2(500, 0, 0, 3), ImageFormat::Rgb10A2).unwrap();
        assert_eq!(report.max_color_delta(), 500);
        assert_eq!(report.histogram[HISTOGRAM_BINS - 1], 1);
    }

    #[test]
    fn rgba8_difference_is_in_eight_bit_codes() {
        let report = compare_image_bytes(&[10, 20, 30, 255], &[12, 20, 29, 255], ImageFormat::Rgba8).unwrap();
        assert_eq!(report.max_delta, [2, 0, 1]);
        assert_eq!(report.differing_at_least_2, 1);
    }

    #[test]
    fn rgba16f_values_are_compared_in_ten_bit_codes() {
        // 1.0 = 0x3C00, 0.5 = 0x3800, 0.0 = 0x0000; 0.5 → код 512 (511,5 округляется вверх), 1.0 → 1023.
        let pixel = |red: u16, green: u16, blue: u16| -> Vec<u8> {
            [red, green, blue, 0x3C00]
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect()
        };
        let report = compare_image_bytes(
            &pixel(0x3C00, 0x3800, 0),
            &pixel(0x3800, 0x3800, 0),
            ImageFormat::Rgba16F,
        )
        .unwrap();
        assert_eq!(report.max_delta, [1023 - 512, 0, 0]);
        // Значение выше единицы обрезается до кода 1023: 2.0 (0x4000) и 1.0 дают одинаковый код.
        let clipped = compare_image_bytes(&pixel(0x4000, 0, 0), &pixel(0x3C00, 0, 0), ImageFormat::Rgba16F).unwrap();
        assert!(clipped.is_identical());
        // Соседние half около 0,5 различаются меньше, чем на код 10-бит: 0x3800 и 0x3801 → одинаковый код.
        let near = compare_image_bytes(&pixel(0x3800, 0, 0), &pixel(0x3801, 0, 0), ImageFormat::Rgba16F).unwrap();
        assert!(near.is_identical());
    }

    #[test]
    fn mismatched_or_ragged_buffers_are_errors_not_silent() {
        assert_eq!(
            compare_image_bytes(&[0; 4], &[0; 8], ImageFormat::Rgba8),
            Err(ImageDiffError::LengthMismatch { first: 4, second: 8 })
        );
        assert!(matches!(
            compare_image_bytes(&[0; 6], &[0; 6], ImageFormat::Rgba8),
            Err(ImageDiffError::NotWholePixels { length: 6, .. })
        ));
        assert!(matches!(
            compare_image_bytes(&[0; 12], &[0; 12], ImageFormat::Rgba16F),
            Err(ImageDiffError::NotWholePixels { .. })
        ));
    }

    #[test]
    fn half_conversion_handles_special_values() {
        assert_eq!(half_bits_to_f32(0x3C00), 1.0);
        assert_eq!(half_bits_to_f32(0xC000), -2.0);
        assert_eq!(half_bits_to_f32(0x0001), 2.0_f32.powi(-24), "наименьшая денормаль");
        assert!(half_bits_to_f32(0x7C00).is_infinite());
        assert!(half_bits_to_f32(0x7E00).is_nan());
        assert_eq!(half_to_ten_bit_code(0x7E00), 0, "NaN → 0, как у выхода ISP");
        assert_eq!(half_to_ten_bit_code(0xBC00), 0, "отрицательное обрезается в 0");
    }
}
