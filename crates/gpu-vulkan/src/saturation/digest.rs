//! Дайджест выхода работы стенда насыщения: «картинка не изменилась» без глаз и без тестовых фич.
//!
//! # Зачем (простыми словами)
//! Оптимизация шейдера обязана не менять картинку. На ПК это доказывают паритетные тесты, но на Adreno другой
//! компилятор шейдеров: то, что бит-в-бит на RADV, на телефоне может разойтись. Поэтому каждая работа стенда
//! после прогона отдаёт короткий «отпечаток» (64 бита) своего итогового выхода; одна и та же сборка обязана
//! давать один и тот же отпечаток от прогона к прогону, а оптимизация — тот же отпечаток, что и «до» неё.
//!
//! # Как считается
//! * хэш — FNV-1a 64 бита (собственная реализация, новых крейтов нет); он НЕ криптографический: нужен только
//!   чтобы заметить любое изменение байт, а не чтобы противостоять подделке;
//! * в хэш входят «паспорт» выхода (формат Vulkan, ширина, высота — чтобы одинаковые байты разной формы не
//!   совпали) и ТОЛЬКО содержимое: у картинки видоискателя ёмкость больше содержимого, а у буфера счётчиков
//!   валидны только ячейки сетки последней записи — хвост за содержимым не определён и в хэш не идёт;
//! * читаются «сырые» байты (10-бит UNORM и fp16 не раскодируются в `f32`: так картинка 4096×3072 не раздувается
//!   до 200 МиБ), блокирующим readback — вне окна замера и после `wait_idle`, поэтому цену работы он не искажает;
//! * выходов у работы может быть несколько (приборы: счётчики и картинка; снимок: картинка и миниатюра) —
//!   каждый получает метку ([`DigestPart`]), а общий отпечаток ([`WorkloadDigest::combined`]) — хэш меток и
//!   отпечатков частей по порядку.
//!
//! # Границы
//! Модуль работает без фич `test-support`: блокирующие чтения лежат в обычных модулях crate. Ошибка чтения —
//! типизированная [`DigestError`]; стенд не должен срывать строку замера из-за отказа дайджеста.

use ash::vk;

use crate::color_target::{ColorTarget, ColorTargetPreviousUse};
use crate::device_buffer::DeviceBuffer;
use crate::error::GpuError;
use crate::rgba8_image::Rgba8StorageImage;
use crate::storage_image::{StorageImage, StorageImagePreviousUse};

/// Начальное значение FNV-1a 64 (offset basis).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// Простое число FNV-1a 64.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64: побайтово «xor, затем умножение».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fnv1a64 {
    state: u64,
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

impl Fnv1a64 {
    /// Пустой хэш (равен [`FNV_OFFSET_BASIS`]).
    pub const fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS,
        }
    }

    /// Подмешивает байты.
    pub fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(FNV_PRIME);
        }
    }

    /// Подмешивает число как 4 байта little-endian (порядок байт не зависит от платформы).
    pub fn update_u32(&mut self, value: u32) {
        self.update(&value.to_le_bytes());
    }

    /// Подмешивает число как 8 байт little-endian.
    pub fn update_u64(&mut self, value: u64) {
        self.update(&value.to_le_bytes());
    }

    /// Итоговое значение (хэш можно продолжать после вызова).
    pub const fn finish(&self) -> OutputDigest {
        OutputDigest(self.state)
    }
}

/// 64-битный отпечаток выхода. В тексте — 16 шестнадцатеричных цифр.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputDigest(u64);

impl OutputDigest {
    /// Из готового значения (разбор сохранённого результата).
    pub const fn from_value(value: u64) -> Self {
        Self(value)
    }

    /// Значение хэша.
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for OutputDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

/// Почему отпечаток не получен.
#[derive(Debug, thiserror::Error)]
pub enum DigestError {
    /// Ошибка GPU при блокирующем чтении.
    #[error("чтение выхода для дайджеста: {0}")]
    Gpu(#[from] GpuError),
    /// Размеры выхода не сходятся с размерами прочитанного (внутренняя ошибка вызывающего).
    #[error(
        "содержимое {content_width}x{content_height} не помещается в прочитанные {read_bytes} байт картинки {capacity_width}x{capacity_height}"
    )]
    ContentOutsideImage {
        /// Ширина содержимого.
        content_width: u32,
        /// Высота содержимого.
        content_height: u32,
        /// Ширина ёмкости картинки.
        capacity_width: u32,
        /// Высота ёмкости картинки.
        capacity_height: u32,
        /// Сколько байт прочитано.
        read_bytes: usize,
    },
    /// Запрошено больше байт буфера, чем в нём есть.
    #[error("запрошено {requested} байт буфера, прочитано {read}")]
    BufferTooShort {
        /// Сколько просили.
        requested: usize,
        /// Сколько прочитано.
        read: usize,
    },
    /// У работы нет выхода, который можно прочитать (ни один кадр не записан).
    #[error("у работы нет выхода для дайджеста: {what}")]
    NothingToRead {
        /// Чего именно нет.
        what: String,
    },
}

/// Отпечаток картинки: паспорт (формат, размеры содержимого) + байты строк содержимого.
fn digest_pixels(
    format: vk::Format,
    capacity: vk::Extent2D,
    content: vk::Extent2D,
    bytes_per_texel: u32,
    bytes: &[u8],
) -> Result<OutputDigest, DigestError> {
    let row_pitch = capacity.width as usize * bytes_per_texel as usize;
    let row_bytes = content.width as usize * bytes_per_texel as usize;
    let outside = || DigestError::ContentOutsideImage {
        content_width: content.width,
        content_height: content.height,
        capacity_width: capacity.width,
        capacity_height: capacity.height,
        read_bytes: bytes.len(),
    };
    if content.width > capacity.width
        || content.height > capacity.height
        || bytes.len() < row_pitch * capacity.height as usize
    {
        return Err(outside());
    }
    let mut hash = Fnv1a64::new();
    hash.update_u32(format.as_raw() as u32);
    hash.update_u32(content.width);
    hash.update_u32(content.height);
    for row in 0..content.height as usize {
        let start = row * row_pitch;
        hash.update(&bytes[start..start + row_bytes]);
    }
    Ok(hash.finish())
}

/// Отпечаток storage-картинки: только `content` (левый верхний угол ёмкости), в раскладке `current_use`.
/// После чтения картинка остаётся в той же раскладке (чтение не меняет состояние, которым владеет вызывающий).
pub fn digest_storage_image(
    image: &StorageImage,
    current_use: StorageImagePreviousUse,
    content: vk::Extent2D,
) -> Result<OutputDigest, DigestError> {
    let bytes = crate::storage_image::readback::read_storage_image_bytes(image, current_use)?;
    digest_pixels(
        image.format().vk_format(),
        image.extent(),
        content,
        image.format().bytes_per_texel(),
        &bytes,
    )
}

/// Отпечаток цветовой цели целиком (кадр показа). Раскладка цели после чтения прежняя.
pub fn digest_color_target(
    target: &ColorTarget,
    current_use: ColorTargetPreviousUse,
) -> Result<OutputDigest, DigestError> {
    let bytes = crate::color_target::readback::read_color_target_bytes(target, current_use)?;
    digest_pixels(
        target.format(),
        target.extent(),
        target.extent(),
        target.target_format().bytes_per_texel(),
        &bytes,
    )
}

/// Отпечаток первых `byte_count` байт device-буфера (счётчики приборов: валидны только ячейки сетки).
/// Контракт чтения — как у `DeviceBuffer::read_back_blocking`: запись GPU завершена.
pub fn digest_device_buffer_prefix(buffer: &DeviceBuffer, byte_count: usize) -> Result<OutputDigest, DigestError> {
    let bytes = buffer.read_back_blocking()?;
    if bytes.len() < byte_count {
        return Err(DigestError::BufferTooShort {
            requested: byte_count,
            read: bytes.len(),
        });
    }
    let mut hash = Fnv1a64::new();
    hash.update_u64(byte_count as u64);
    hash.update(&bytes[..byte_count]);
    Ok(hash.finish())
}

/// Отпечаток RGBA8-картинки снимка целиком (плотные строки). Картинка уже в раскладке чтения читателем
/// (контракт `Rgba8StorageImage::read_back_blocking`).
pub fn digest_rgba8_image(image: &Rgba8StorageImage) -> Result<OutputDigest, DigestError> {
    let bytes = image.read_back_blocking()?;
    Ok(digest_rgba8_bytes(image.extent(), &bytes))
}

/// Отпечаток RGBA8 по готовым байтам (плотные строки `extent`): картинка снимка и его миниатюра.
pub fn digest_rgba8_bytes(extent: vk::Extent2D, bytes: &[u8]) -> OutputDigest {
    let mut hash = Fnv1a64::new();
    hash.update_u32(vk::Format::R8G8B8A8_UNORM.as_raw() as u32);
    hash.update_u32(extent.width);
    hash.update_u32(extent.height);
    hash.update(bytes);
    hash.finish()
}

/// Отпечатки по нескольким числам (счётчики вида «ячейка → число»): порядок ячеек — часть отпечатка.
pub fn digest_counts(counts: &[u32]) -> OutputDigest {
    let mut hash = Fnv1a64::new();
    hash.update_u64(counts.len() as u64);
    for count in counts {
        hash.update_u32(*count);
    }
    hash.finish()
}

/// Отпечаток картинки из `f32`-текселей (картинка прибора после раскодирования): биты, а не значения —
/// `-0.0` и `0.0` различаются.
pub fn digest_texels(width: u32, height: u32, texels: &[[f32; 4]]) -> OutputDigest {
    let mut hash = Fnv1a64::new();
    hash.update_u32(width);
    hash.update_u32(height);
    for texel in texels {
        for channel in texel {
            hash.update_u32(channel.to_bits());
        }
    }
    hash.finish()
}

/// Одна часть выхода работы с меткой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigestPart {
    /// Что это за выход (`image`, `counters`, `picture`, `thumbnail`, `target`...).
    pub label: &'static str,
    /// Отпечаток части.
    pub digest: OutputDigest,
}

/// Отпечаток всего выхода работы: части по порядку.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkloadDigest {
    parts: Vec<DigestPart>,
}

impl WorkloadDigest {
    /// Из частей (порядок — часть отпечатка).
    pub fn from_parts(parts: Vec<DigestPart>) -> Self {
        Self { parts }
    }

    /// Выход из одной части.
    pub fn single(label: &'static str, digest: OutputDigest) -> Self {
        Self::from_parts(vec![DigestPart { label, digest }])
    }

    /// Части по порядку.
    pub fn parts(&self) -> &[DigestPart] {
        &self.parts
    }

    /// Общий отпечаток: хэш (метка, 0, отпечаток части) по порядку. Меняется при смене любой части.
    pub fn combined(&self) -> OutputDigest {
        let mut hash = Fnv1a64::new();
        for part in &self.parts {
            hash.update(part.label.as_bytes());
            hash.update(&[0]);
            hash.update_u64(part.digest.value());
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_hashes_to_the_published_offset_basis() {
        assert_eq!(Fnv1a64::new().finish().value(), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn matches_published_fnv1a_64_test_vectors() {
        // Векторы из спецификации FNV (http://www.isthe.com/chongo/src/fnv/test_fnv.c).
        for (input, expected) in [("a", 0xaf63_dc4c_8601_ec8c_u64), ("foobar", 0x8594_4171_f739_67e8)] {
            let mut hash = Fnv1a64::new();
            hash.update(input.as_bytes());
            assert_eq!(hash.finish().value(), expected, "FNV-1a 64 от {input:?}");
        }
    }

    #[test]
    fn digest_text_is_sixteen_lowercase_hex_digits() {
        assert_eq!(OutputDigest::from_value(0xab).to_string(), "00000000000000ab");
    }

    #[test]
    fn pixel_digest_ignores_capacity_tail_but_sees_content_changes_and_shape() {
        let capacity = vk::Extent2D { width: 4, height: 3 };
        let content = vk::Extent2D { width: 2, height: 2 };
        let format = vk::Format::A2B10G10R10_UNORM_PACK32;
        let mut bytes = vec![0_u8; 4 * 3 * 4];
        let base = digest_pixels(format, capacity, content, 4, &bytes).expect("содержимое внутри ёмкости");
        // Хвост ёмкости (колонки 2..4 и строка 2) в отпечаток не входит.
        bytes[2 * 4] = 0xff;
        bytes[2 * 4 * 4] = 0xff;
        assert_eq!(digest_pixels(format, capacity, content, 4, &bytes).unwrap(), base);
        // Пиксель содержимого входит.
        bytes[4] = 1;
        assert_ne!(digest_pixels(format, capacity, content, 4, &bytes).unwrap(), base);
        // Те же байты другой формы — другой отпечаток.
        let narrow = vk::Extent2D { width: 1, height: 4 };
        let zero = vec![0_u8; 4 * 4 * 4];
        let wide = digest_pixels(
            format,
            vk::Extent2D { width: 4, height: 4 },
            vk::Extent2D { width: 4, height: 1 },
            4,
            &zero,
        )
        .unwrap();
        let tall = digest_pixels(format, vk::Extent2D { width: 4, height: 4 }, narrow, 4, &zero).unwrap();
        assert_ne!(wide, tall);
    }

    #[test]
    fn content_larger_than_capacity_is_a_typed_error() {
        let error = digest_pixels(
            vk::Format::R8G8B8A8_UNORM,
            vk::Extent2D { width: 2, height: 2 },
            vk::Extent2D { width: 3, height: 1 },
            4,
            &[0; 16],
        )
        .unwrap_err();
        assert!(matches!(error, DigestError::ContentOutsideImage { .. }), "{error:?}");
    }

    #[test]
    fn combined_digest_depends_on_every_part_label_and_order() {
        let first = DigestPart {
            label: "counters",
            digest: OutputDigest::from_value(1),
        };
        let second = DigestPart {
            label: "picture",
            digest: OutputDigest::from_value(2),
        };
        let both = WorkloadDigest::from_parts(vec![first, second]);
        let swapped = WorkloadDigest::from_parts(vec![second, first]);
        let changed = WorkloadDigest::from_parts(vec![
            first,
            DigestPart {
                digest: OutputDigest::from_value(3),
                ..second
            },
        ]);
        assert_ne!(both.combined(), swapped.combined());
        assert_ne!(both.combined(), changed.combined());
        assert_eq!(both.parts().len(), 2);
    }

    #[test]
    fn counts_digest_depends_on_order_and_length() {
        assert_ne!(digest_counts(&[1, 2]), digest_counts(&[2, 1]));
        assert_ne!(digest_counts(&[1, 0]), digest_counts(&[1]));
    }
}
